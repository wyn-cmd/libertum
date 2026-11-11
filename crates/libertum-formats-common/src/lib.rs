//! Code shared by the format crates (PLAN §6.4): XML helpers, lengths and colours.
//!
//! The two document formats both need a small streaming XML reader, the same ODF length
//! syntax (points, centimetres, millimetres, inches, picas, pixels) and straight RGBA colours.
//! Keeping them here stops `libertum-docx` and `libertum-odt` from growing two subtly
//! different copies. It also turns image bytes into the model's `Media` ([`media`]). This crate
//! depends on the model, never on a UI crate.

pub mod colour;
pub mod media;
pub mod units;
pub mod xml;
