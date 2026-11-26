//! `libertum-audio`: plays one audio clip at a time for the document's music players.
//!
//! - [`decode`] reads MP3, OGG (Vorbis), WAV, FLAC and M4A (AAC) from memory with Symphonia,
//!   and can seek.
//! - [`output`] is the sound device. On Linux it loads the system's PulseAudio client library
//!   (which PipeWire also answers) or else ALSA at run time, so building needs no sound headers.
//!   The device is open only while a clip is playing, so a paused or finished player holds
//!   nothing and a closed document leaves nothing behind.
//! - [`player`] runs a clip on its own thread and shows its state to the interface through a
//!   cheap snapshot.

pub mod decode;
pub mod output;
pub mod player;

pub use decode::{Clip, DecodeError};
pub use output::{Opener, Sink, system_opener};
pub use player::{Player, Snapshot};
