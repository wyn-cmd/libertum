//! Text decoding for RTF: `\ansicpg`, `\uc` fallbacks and `\uN` characters (PLAN §6.5).
//!
//! An RTF stream is meant to be 7-bit. Characters arrive either as `\uN` (a signed 16-bit
//! UTF-16 code unit) or as raw bytes under the document's ANSI code page. `\uN` is decoded
//! directly; raw bytes and `\'hh` bytes go through a streaming [`encoding_rs`] decoder, so a
//! multi-byte code page (UTF-8, Shift_JIS, GBK, ...) is decoded correctly even when its bytes
//! are split between text tokens and `\'hh` escapes.
//!
//! `\ucN` says how many fallback characters follow each `\uN`; the caller skips them with
//! [`TextDecoder::skip_fallback`]. Surrogate pairs are combined here, and a lone high
//! surrogate becomes U+FFFD at the end of a paragraph so no invalid scalar reaches the model.

use encoding_rs::{
    BIG5, Decoder, DecoderResult, EUC_KR, Encoding, GBK, MACINTOSH, SHIFT_JIS, UTF_8, WINDOWS_874,
    WINDOWS_1250, WINDOWS_1251, WINDOWS_1252, WINDOWS_1253, WINDOWS_1254, WINDOWS_1255,
    WINDOWS_1256, WINDOWS_1257, WINDOWS_1258,
};

/// The replacement character used for malformed input and unpaired surrogates.
const REPLACEMENT: char = '\u{FFFD}';

/// A streaming decoder for one ANSI code page, plus the pending half of a surrogate pair.
pub(crate) struct TextDecoder {
    encoding: &'static Encoding,
    decoder: Decoder,
    codepage: u16,
    pending_high: Option<u16>,
}

impl TextDecoder {
    /// A decoder for `codepage`. The second value is the code page actually used when the
    /// requested one is not available, so the caller can warn about the fallback.
    pub(crate) fn new(codepage: u16) -> (Self, Option<u16>) {
        let (encoding, fallback) = encoding_for(codepage);
        (
            Self {
                encoding,
                decoder: encoding.new_decoder_without_bom_handling(),
                codepage,
                pending_high: None,
            },
            fallback,
        )
    }

    pub(crate) fn codepage(&self) -> u16 {
        self.codepage
    }

    /// Changes the code page, discarding any incomplete byte sequence. Returns the fallback
    /// code page when the requested one is unknown.
    pub(crate) fn set_codepage(&mut self, codepage: u16) -> Option<u16> {
        let (encoding, fallback) = encoding_for(codepage);
        self.encoding = encoding;
        self.decoder = encoding.new_decoder_without_bom_handling();
        self.codepage = codepage;
        fallback
    }

    /// Decodes a complete chunk of raw bytes. Bytes that begin a multi-byte character are
    /// held in the decoder until the next call.
    pub(crate) fn decode(&mut self, bytes: &[u8]) -> String {
        if bytes.is_empty() {
            return String::new();
        }
        // The overwhelmingly common case is ASCII text in a single-byte code page; keep it
        // allocation-light and leave the decoder alone. Multi-byte pages must go through the
        // decoder even for ASCII, because it may hold an incomplete earlier sequence.
        if self.encoding.is_single_byte() && bytes.iter().all(|byte| *byte < 0x80) {
            return bytes.iter().map(|byte| char::from(*byte)).collect();
        }
        self.decode_slow(bytes, false)
    }

    fn decode_slow(&mut self, bytes: &[u8], last: bool) -> String {
        let mut out = String::new();
        let mut read = 0usize;
        loop {
            let remaining = bytes.len().saturating_sub(read);
            let capacity = self
                .decoder
                .max_utf8_buffer_length_without_replacement(remaining)
                .unwrap_or(remaining.saturating_mul(4) + 16)
                .max(16);
            out.reserve(capacity);
            let (result, consumed) =
                self.decoder
                    .decode_to_string_without_replacement(&bytes[read..], &mut out, last);
            read += consumed;
            match result {
                DecoderResult::InputEmpty => break,
                DecoderResult::OutputFull => continue,
                DecoderResult::Malformed(..) => {
                    out.push(REPLACEMENT);
                    if consumed == 0 {
                        // Defensive: never loop without progress on malformed input.
                        break;
                    }
                }
            }
        }
        out
    }

    /// Flushes an incomplete byte sequence, leaving a pending surrogate alone.
    pub(crate) fn flush_bytes(&mut self) -> String {
        let out = self.decode_slow(&[], true);
        self.decoder = self.encoding.new_decoder_without_bom_handling();
        out
    }

    /// Flushes an incomplete sequence and any pending surrogate, then starts a fresh decoder.
    pub(crate) fn finish(&mut self) -> String {
        let mut out = self.flush_bytes();
        if let Some(character) = self.take_pending() {
            out.push(character);
        }
        out
    }

    /// Feeds one UTF-16 code unit from `\uN`, combining a surrogate pair when the next unit
    /// completes it. Returns the text to append (possibly just U+FFFD for an unpaired unit).
    pub(crate) fn push_unit(&mut self, unit: u16) -> String {
        let mut out = String::new();
        if let Some(high) = self.pending_high.take() {
            if (0xDC00..=0xDFFF).contains(&unit) {
                let scalar =
                    0x1_0000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
                out.push(char::from_u32(scalar).unwrap_or(REPLACEMENT));
                return out;
            }
            out.push(REPLACEMENT);
        }
        if (0xD800..=0xDBFF).contains(&unit) {
            self.pending_high = Some(unit);
        } else if (0xDC00..=0xDFFF).contains(&unit) {
            out.push(REPLACEMENT);
        } else if let Some(character) = char::from_u32(u32::from(unit)) {
            out.push(character);
        } else {
            out.push(REPLACEMENT);
        }
        out
    }

    /// The pending high surrogate as a replacement character, if any.
    pub(crate) fn take_pending(&mut self) -> Option<char> {
        self.pending_high.take().map(|_| REPLACEMENT)
    }
}

/// The `encoding_rs` encoding for an RTF ANSI code page (Windows or DOS), and the code page
/// that stood in when it is not known. CP1252 is the fallback because it is RTF's default.
pub(crate) fn encoding_for(codepage: u16) -> (&'static Encoding, Option<u16>) {
    let encoding = match codepage {
        0 | 1252 => Some(WINDOWS_1252),
        65001 => Some(UTF_8),
        932 | 943 => Some(SHIFT_JIS),
        936 | 54936 => Some(GBK),
        949 => Some(EUC_KR),
        950 => Some(BIG5),
        874 => Some(WINDOWS_874),
        10000 => Some(MACINTOSH),
        1250 => Some(WINDOWS_1250),
        1251 => Some(WINDOWS_1251),
        1253 => Some(WINDOWS_1253),
        1254 => Some(WINDOWS_1254),
        1255 => Some(WINDOWS_1255),
        1256 => Some(WINDOWS_1256),
        1257 => Some(WINDOWS_1257),
        1258 => Some(WINDOWS_1258),
        _ => Encoding::for_label(format!("windows-{codepage}").as_bytes())
            .or_else(|| Encoding::for_label(format!("cp{codepage}").as_bytes())),
    };
    match encoding {
        Some(encoding) => (encoding, None),
        None => (WINDOWS_1252, Some(1252)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp1252_high_bytes_become_the_right_characters() {
        let (mut decoder, fallback) = TextDecoder::new(1252);
        assert_eq!(fallback, None);
        assert_eq!(decoder.decode(&[0x93]), "\u{201C}");
        assert_eq!(decoder.decode(b"caf\xE9"), "café");
        assert_eq!(decoder.codepage(), 1252);
    }

    #[test]
    fn unicode_units_combine_into_a_surrogate_pair() {
        let (mut decoder, _) = TextDecoder::new(1252);
        assert_eq!(decoder.push_unit(0xD83D), "");
        assert_eq!(decoder.push_unit(0xDE00), "\u{1F600}");
        assert_eq!(decoder.push_unit(0x2022), "\u{2022}");
    }

    #[test]
    fn a_lone_surrogate_becomes_the_replacement_character() {
        let (mut decoder, _) = TextDecoder::new(1252);
        assert_eq!(decoder.push_unit(0xD83D), "");
        assert_eq!(decoder.push_unit(0x2022), "\u{FFFD}\u{2022}");
        assert_eq!(decoder.push_unit(0xDE00), "\u{FFFD}");
        assert_eq!(decoder.take_pending(), None);
    }

    #[test]
    fn utf8_bytes_split_across_calls_are_decoded() {
        let (mut decoder, fallback) = TextDecoder::new(65001);
        assert_eq!(fallback, None);
        // "€" is E2 82 AC.
        assert_eq!(decoder.decode(&[0xE2]), "");
        assert_eq!(decoder.decode(&[0x82]), "");
        assert_eq!(decoder.decode(&[0xAC]), "\u{20AC}");
        assert_eq!(decoder.decode("café".as_bytes()), "café");
    }

    #[test]
    fn an_unknown_code_page_falls_back_to_cp1252() {
        let (_, fallback) = TextDecoder::new(437);
        assert_eq!(fallback, Some(1252));
    }

    #[test]
    fn finish_flushes_an_incomplete_sequence_and_a_pending_surrogate() {
        let (mut decoder, _) = TextDecoder::new(65001);
        assert_eq!(decoder.decode(&[0xE2]), "");
        assert_eq!(decoder.finish(), "\u{FFFD}");
        let (mut decoder, _) = TextDecoder::new(1252);
        assert_eq!(decoder.push_unit(0xD83D), "");
        assert_eq!(decoder.finish(), "\u{FFFD}");
    }
}
