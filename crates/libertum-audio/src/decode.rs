//! Decoding a clip held in memory.

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::Duration;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("this is not audio Libertum can play")]
    Unsupported,
    #[error("the audio is damaged")]
    Damaged,
}

/// The bytes of the file, shared with whoever else holds them.
struct Bytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

struct Source(Cursor<Bytes>);

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Seek for Source {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

impl MediaSource for Source {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.0.get_ref().0.len() as u64)
    }
}

/// An open clip, read a chunk of interleaved samples at a time.
pub struct Clip {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// How long it is, when the file says (an MP3 without a length header does not).
    pub duration: Option<Duration>,
    time_base: Option<TimeBase>,
    buffer: Option<SampleBuffer<f32>>,
}

impl Clip {
    /// Opens the file. `extension` (`mp3`, `ogg`...) only helps the format guess.
    pub fn open(bytes: Arc<Vec<u8>>, extension: &str) -> Result<Self, DecodeError> {
        let stream = MediaSourceStream::new(
            Box::new(Source(Cursor::new(Bytes(bytes)))),
            Default::default(),
        );
        let mut hint = Hint::new();
        hint.with_extension(extension);
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|_| DecodeError::Unsupported)?;
        let format = probed.format;
        let track = format.default_track().ok_or(DecodeError::Unsupported)?;
        let params = track.codec_params.clone();
        let id = track.id;
        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .map_err(|_| DecodeError::Unsupported)?;
        let sample_rate = params.sample_rate.ok_or(DecodeError::Unsupported)?;
        let channels = params
            .channels
            .map(|c| c.count() as u16)
            .ok_or(DecodeError::Unsupported)?;
        let duration = params
            .n_frames
            .map(|frames| Duration::from_secs_f64(frames as f64 / f64::from(sample_rate)));
        Ok(Self {
            format,
            decoder,
            track: id,
            sample_rate,
            channels,
            duration,
            time_base: params.time_base,
            buffer: None,
        })
    }

    /// The next stretch of interleaved samples, or `None` at the end. A packet that cannot
    /// be decoded is skipped; a damaged file ends the clip rather than stopping the app.
    pub fn next_chunk(&mut self) -> Option<Vec<f32>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(_) => return None,
            };
            if packet.track_id() != self.track {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    let needed = decoded.capacity() as u64;
                    let buffer = match &mut self.buffer {
                        Some(buffer)
                            if buffer.capacity() as u64
                                >= needed * u64::from(spec.channels.count() as u32) =>
                        {
                            buffer
                        }
                        slot => slot.insert(SampleBuffer::<f32>::new(needed, spec)),
                    };
                    buffer.copy_interleaved_ref(decoded);
                    let samples = buffer.samples();
                    if samples.is_empty() {
                        continue;
                    }
                    return Some(samples.to_vec());
                }
                Err(Error::DecodeError(_)) | Err(Error::IoError(_)) => continue,
                Err(_) => return None,
            }
        }
    }

    /// Moves to `to`. Returns where it actually landed.
    pub fn seek(&mut self, to: Duration) -> Duration {
        let target = match self.duration {
            Some(total) => to.min(total),
            None => to,
        };
        match self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from(target.as_secs_f64()),
                track_id: Some(self.track),
            },
        ) {
            Ok(done) => {
                self.decoder.reset();
                // `actual_ts` is in the track's time base (the sample rate when it has none).
                match self.time_base {
                    Some(base) => {
                        let time = base.calc_time(done.actual_ts);
                        Duration::from_secs_f64(time.seconds as f64 + time.frac)
                    }
                    None => {
                        Duration::from_secs_f64(done.actual_ts as f64 / f64::from(self.sample_rate))
                    }
                }
            }
            Err(_) => Duration::ZERO,
        }
    }
}
