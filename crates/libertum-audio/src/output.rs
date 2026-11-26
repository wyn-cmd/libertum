//! The sound device.

/// A device playing interleaved `f32` samples. Writing blocks until the device has room, which
/// is what paces a clip to real time.
pub trait Sink: Send {
    fn write(&mut self, samples: &[f32]) -> Result<(), String>;
    /// Frames written but not yet heard.
    fn latency_frames(&mut self) -> u64;
    /// Throws away what is queued, so a pause or a seek is heard at once.
    fn flush(&mut self);
}

/// Opens a device for a sample rate and channel count.
pub type Opener = std::sync::Arc<dyn Fn(u32, u16) -> Result<Box<dyn Sink>, String> + Send + Sync>;

/// The system's own output.
pub fn system_opener() -> Opener {
    std::sync::Arc::new(imp::open)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Sink;
    use std::ffi::{CString, c_char, c_int, c_void};

    /// A shared library loaded at run time and closed again with the device.
    struct Library(*mut c_void);

    // SAFETY: the handle is only used to look up symbols and to close it, from one thread at a
    // time (a device is owned by the player's single thread).
    unsafe impl Send for Library {}

    impl Library {
        fn open(names: &[&str]) -> Option<Self> {
            for name in names {
                let name = CString::new(*name).ok()?;
                // SAFETY: a valid C string; dlopen has no other preconditions.
                let handle =
                    unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
                if !handle.is_null() {
                    return Some(Self(handle));
                }
            }
            None
        }

        fn symbol<T: Copy>(&self, name: &str) -> Option<T> {
            let name = CString::new(name).ok()?;
            // SAFETY: `self.0` is a live handle; the caller names a function type `T` of the
            // pointer's size (checked below) that matches the C declaration.
            let address = unsafe { libc::dlsym(self.0, name.as_ptr()) };
            if address.is_null() || std::mem::size_of::<T>() != std::mem::size_of::<*mut c_void>() {
                return None;
            }
            // SAFETY: same size as a pointer, and the symbol is a function of that type.
            Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&address) })
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            // SAFETY: a handle from dlopen, closed once.
            unsafe { libc::dlclose(self.0) };
        }
    }

    pub fn open(rate: u32, channels: u16) -> Result<Box<dyn Sink>, String> {
        match Pulse::open(rate, channels) {
            Ok(pulse) => Ok(Box::new(pulse)),
            Err(pulse_error) => match Alsa::open(rate, channels) {
                Ok(alsa) => Ok(Box::new(alsa)),
                Err(alsa_error) => Err(format!("{pulse_error}; {alsa_error}")),
            },
        }
    }

    // ---- PulseAudio's simple API (PipeWire serves it too) ----

    #[repr(C)]
    struct SampleSpec {
        format: c_int,
        rate: u32,
        channels: u8,
    }

    /// `PA_SAMPLE_FLOAT32LE`.
    const PA_SAMPLE_FLOAT32LE: c_int = 5;
    /// `PA_STREAM_PLAYBACK`.
    const PA_STREAM_PLAYBACK: c_int = 1;

    /// `pa_buffer_attr`. A short buffer (a quarter of a second) keeps pause and seek quick; the
    /// default is two seconds, which would play on after Pause.
    #[repr(C)]
    struct BufferAttr {
        maxlength: u32,
        tlength: u32,
        prebuf: u32,
        minreq: u32,
        fragsize: u32,
    }

    type New = unsafe extern "C" fn(
        *const c_char,
        *const c_char,
        c_int,
        *const c_char,
        *const c_char,
        *const SampleSpec,
        *const c_void,
        *const c_void,
        *mut c_int,
    ) -> *mut c_void;
    type Write = unsafe extern "C" fn(*mut c_void, *const c_void, usize, *mut c_int) -> c_int;
    type Flush = unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int;
    type Latency = unsafe extern "C" fn(*mut c_void, *mut c_int) -> u64;
    type Free = unsafe extern "C" fn(*mut c_void);

    struct Pulse {
        stream: *mut c_void,
        write: Write,
        flush: Flush,
        latency: Latency,
        free: Free,
        rate: u32,
        channels: u16,
        _library: Library,
    }

    // SAFETY: the stream is used from the one thread that owns the Pulse.
    unsafe impl Send for Pulse {}

    impl Pulse {
        fn open(rate: u32, channels: u16) -> Result<Self, String> {
            let library = Library::open(&["libpulse-simple.so.0", "libpulse-simple.so"])
                .ok_or("the PulseAudio library is not installed")?;
            let new: New = library
                .symbol("pa_simple_new")
                .ok_or("pa_simple_new is missing")?;
            let write: Write = library
                .symbol("pa_simple_write")
                .ok_or("pa_simple_write is missing")?;
            let flush: Flush = library
                .symbol("pa_simple_flush")
                .ok_or("pa_simple_flush is missing")?;
            let latency: Latency = library
                .symbol("pa_simple_get_latency")
                .ok_or("pa_simple_get_latency is missing")?;
            let free: Free = library
                .symbol("pa_simple_free")
                .ok_or("pa_simple_free is missing")?;
            let spec = SampleSpec {
                format: PA_SAMPLE_FLOAT32LE,
                rate,
                channels: u8::try_from(channels).map_err(|_| "too many channels")?,
            };
            let name = CString::new("Libertum").map_err(|_| "bad name")?;
            let stream_name = CString::new("Music").map_err(|_| "bad name")?;
            let mut error: c_int = 0;
            let quarter_second = (rate / 4).max(1) * u32::from(channels) * 4;
            let attr = BufferAttr {
                maxlength: u32::MAX,
                tlength: quarter_second,
                prebuf: u32::MAX,
                minreq: u32::MAX,
                fragsize: u32::MAX,
            };
            // SAFETY: valid C strings, spec and buffer attributes; null for the server, device
            // and channel map asks for the defaults.
            let stream = unsafe {
                new(
                    std::ptr::null(),
                    name.as_ptr(),
                    PA_STREAM_PLAYBACK,
                    std::ptr::null(),
                    stream_name.as_ptr(),
                    &spec,
                    std::ptr::null(),
                    std::ptr::from_ref(&attr).cast(),
                    &mut error,
                )
            };
            if stream.is_null() {
                return Err(format!(
                    "could not open the sound server (PulseAudio error {error})"
                ));
            }
            Ok(Self {
                stream,
                write,
                flush,
                latency,
                free,
                rate,
                channels,
                _library: library,
            })
        }
    }

    impl Sink for Pulse {
        fn write(&mut self, samples: &[f32]) -> Result<(), String> {
            let mut error: c_int = 0;
            // SAFETY: a live stream and a slice that is valid for its length in bytes.
            let status = unsafe {
                (self.write)(
                    self.stream,
                    samples.as_ptr().cast(),
                    std::mem::size_of_val(samples),
                    &mut error,
                )
            };
            if status < 0 {
                Err(format!(
                    "the sound server stopped answering (error {error})"
                ))
            } else {
                Ok(())
            }
        }

        fn latency_frames(&mut self) -> u64 {
            let mut error: c_int = 0;
            // SAFETY: a live stream.
            let microseconds = unsafe { (self.latency)(self.stream, &mut error) };
            // An error comes back as -1 (all bits set): count it as no latency.
            if microseconds == u64::MAX {
                0
            } else {
                microseconds * u64::from(self.rate) / 1_000_000
            }
        }

        fn flush(&mut self) {
            let mut error: c_int = 0;
            // SAFETY: a live stream.
            unsafe { (self.flush)(self.stream, &mut error) };
        }
    }

    impl Drop for Pulse {
        fn drop(&mut self) {
            let _ = self.channels;
            // SAFETY: a live stream, freed once. It is flushed first so nothing queued is left
            // to play (or click) after the player has gone.
            unsafe {
                let mut error: c_int = 0;
                (self.flush)(self.stream, &mut error);
                (self.free)(self.stream);
            }
        }
    }

    // ---- ALSA, for a system with no PulseAudio client library ----

    /// `SND_PCM_FORMAT_FLOAT_LE`.
    const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;
    const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;

    type AlsaOpen = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
    type AlsaParams =
        unsafe extern "C" fn(*mut c_void, c_int, c_int, u32, u32, c_int, u32) -> c_int;
    type AlsaWrite =
        unsafe extern "C" fn(*mut c_void, *const c_void, std::ffi::c_ulong) -> std::ffi::c_long;
    type AlsaPrepare = unsafe extern "C" fn(*mut c_void) -> c_int;
    type AlsaDrop = unsafe extern "C" fn(*mut c_void) -> c_int;
    type AlsaDelay = unsafe extern "C" fn(*mut c_void, *mut std::ffi::c_long) -> c_int;
    type AlsaClose = unsafe extern "C" fn(*mut c_void) -> c_int;

    struct Alsa {
        pcm: *mut c_void,
        write: AlsaWrite,
        prepare: AlsaPrepare,
        drop_frames: AlsaDrop,
        delay: AlsaDelay,
        close: AlsaClose,
        channels: usize,
        _library: Library,
    }

    // SAFETY: the handle is used from the one thread that owns the Alsa.
    unsafe impl Send for Alsa {}

    impl Alsa {
        fn open(rate: u32, channels: u16) -> Result<Self, String> {
            let library = Library::open(&["libasound.so.2", "libasound.so"])
                .ok_or("the ALSA library is not installed")?;
            let open: AlsaOpen = library
                .symbol("snd_pcm_open")
                .ok_or("snd_pcm_open is missing")?;
            let params: AlsaParams = library
                .symbol("snd_pcm_set_params")
                .ok_or("snd_pcm_set_params is missing")?;
            let write: AlsaWrite = library
                .symbol("snd_pcm_writei")
                .ok_or("snd_pcm_writei is missing")?;
            let prepare: AlsaPrepare = library
                .symbol("snd_pcm_prepare")
                .ok_or("snd_pcm_prepare is missing")?;
            let drop_frames: AlsaDrop = library
                .symbol("snd_pcm_drop")
                .ok_or("snd_pcm_drop is missing")?;
            let delay: AlsaDelay = library
                .symbol("snd_pcm_delay")
                .ok_or("snd_pcm_delay is missing")?;
            let close: AlsaClose = library
                .symbol("snd_pcm_close")
                .ok_or("snd_pcm_close is missing")?;
            let name = CString::new("default").map_err(|_| "bad name")?;
            let mut pcm: *mut c_void = std::ptr::null_mut();
            // SAFETY: a valid out-pointer and C string; 0 is SND_PCM_STREAM_PLAYBACK.
            let status = unsafe { open(&mut pcm, name.as_ptr(), 0, 0) };
            if status < 0 || pcm.is_null() {
                return Err(format!(
                    "could not open the sound device (ALSA error {status})"
                ));
            }
            // SAFETY: a live pcm; resampling allowed, 200 ms of latency.
            let status = unsafe {
                params(
                    pcm,
                    SND_PCM_FORMAT_FLOAT_LE,
                    SND_PCM_ACCESS_RW_INTERLEAVED,
                    u32::from(channels),
                    rate,
                    1,
                    200_000,
                )
            };
            if status < 0 {
                // SAFETY: closes the pcm just opened.
                unsafe { close(pcm) };
                return Err(format!(
                    "the sound device refused the format (ALSA error {status})"
                ));
            }
            Ok(Self {
                pcm,
                write,
                prepare,
                drop_frames,
                delay,
                close,
                channels: usize::from(channels),
                _library: library,
            })
        }
    }

    impl Sink for Alsa {
        fn write(&mut self, samples: &[f32]) -> Result<(), String> {
            let mut left = samples;
            let mut failures = 0;
            while !left.is_empty() {
                let frames = left.len() / self.channels;
                if frames == 0 {
                    break;
                }
                // SAFETY: a live pcm and a buffer holding at least `frames` frames.
                let wrote = unsafe {
                    (self.write)(self.pcm, left.as_ptr().cast(), frames as std::ffi::c_ulong)
                };
                if wrote < 0 {
                    // An underrun (-EPIPE) or a suspend: prepare again and retry once.
                    // SAFETY: a live pcm.
                    let status = unsafe { (self.prepare)(self.pcm) };
                    failures += 1;
                    if status < 0 || failures > 3 {
                        return Err(format!(
                            "the sound device stopped answering (ALSA error {status})"
                        ));
                    }
                    continue;
                }
                left = &left[(wrote as usize * self.channels).min(left.len())..];
            }
            Ok(())
        }

        fn latency_frames(&mut self) -> u64 {
            let mut frames: std::ffi::c_long = 0;
            // SAFETY: a live pcm and a valid out-pointer.
            let status = unsafe { (self.delay)(self.pcm, &mut frames) };
            if status < 0 { 0 } else { frames.max(0) as u64 }
        }

        fn flush(&mut self) {
            // SAFETY: a live pcm; the next write needs it prepared again.
            unsafe {
                (self.drop_frames)(self.pcm);
                (self.prepare)(self.pcm);
            }
        }
    }

    impl Drop for Alsa {
        fn drop(&mut self) {
            // SAFETY: a live pcm, dropped (not drained: nothing queued should play on) and closed once.
            unsafe {
                (self.drop_frames)(self.pcm);
                (self.close)(self.pcm);
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::Sink;

    pub fn open(_rate: u32, _channels: u16) -> Result<Box<dyn Sink>, String> {
        Err("playing music is not available on this system yet".to_owned())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// Opens the machine's real sound output and plays half a second of silence. Needs a sound
    /// server, so it is run by hand: `cargo test -p libertum-audio -- --ignored`.
    #[test]
    #[ignore = "needs a sound server"]
    fn the_system_output_opens_and_takes_silence() {
        let mut sink = system_opener()(44_100, 2).expect("a sound output");
        let silence = vec![0.0_f32; 44_100];
        sink.write(&silence).expect("write");
        let _ = sink.latency_frames();
        sink.flush();
    }
}
