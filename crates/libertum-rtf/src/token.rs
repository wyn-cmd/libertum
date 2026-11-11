//! The byte-level RTF tokeniser (PLAN §6.5).
//!
//! RTF is one stream of groups, control words, control symbols, hex bytes and text. This
//! tokeniser never builds a tree: it hands out borrowed tokens with their source offsets, so
//! the reader can map what it understands and copy the raw bytes of what it does not without
//! holding the document twice.
//!
//! Control-word names are ASCII letters by definition, so they are borrowed as `&str` without
//! allocation. Parameters are signed 32-bit values; an overflowing or malformed parameter is a
//! typed error rather than a panic or a silently truncated number.

use crate::RtfError;

/// One token of an RTF stream, borrowed from the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Token<'a> {
    /// `{`
    GroupStart,
    /// `}`
    GroupEnd,
    /// `\word` with an optional signed parameter. A single delimiter space has been consumed.
    ControlWord { name: &'a str, param: Option<i32> },
    /// A non-alphabetic control symbol such as `\*`, `\\`, `\{`, `\~` or `\-`.
    ControlSymbol(u8),
    /// `\'hh`: one byte written as two hex digits.
    HexByte(u8),
    /// A run of text. CR and LF between tokens are skipped, as the RTF specification allows.
    Text(&'a [u8]),
}

/// A cursor over an RTF byte stream.
pub(crate) struct Tokenizer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        let mut pos = 0usize;
        // A UTF-8 BOM in front of an RTF stream is not meaningful, but some writers add one.
        if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            pos = 3;
        }
        Self { bytes, pos }
    }

    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    pub(crate) fn set_position(&mut self, pos: usize) {
        self.pos = pos.min(self.bytes.len());
    }

    pub(crate) fn input(&self) -> &'a [u8] {
        self.bytes
    }

    /// The whole raw group beginning at `start` (which must be a `{`), with brace nesting and
    /// backslash escapes respected. The current position is moved past the matching `}`.
    pub(crate) fn take_raw_group(&mut self, start: usize) -> Result<&'a [u8], RtfError> {
        // Callers record the cursor before asking for the next token, so `start` may point at
        // CR/LF (which the tokeniser skips) rather than at the brace itself.
        let mut start = start.min(self.bytes.len());
        while start < self.bytes.len() && matches!(self.bytes[start], b' ' | b'\t' | b'\r' | b'\n')
        {
            start += 1;
        }
        let end = self.raw_group_end(start)?;
        self.pos = end;
        Ok(&self.bytes[start..end])
    }

    /// The end offset (one past the closing `}`) of the group beginning at `start`.
    pub(crate) fn raw_group_end(&self, start: usize) -> Result<usize, RtfError> {
        if self.bytes.get(start) != Some(&b'{') {
            return Err(RtfError::parse(start, "expected a group"));
        }
        let mut depth = 0usize;
        let mut i = start;
        while i < self.bytes.len() {
            match self.bytes[i] {
                b'\\' => {
                    if i + 1 >= self.bytes.len() {
                        return Err(RtfError::parse(i, "a backslash ends the stream"));
                    }
                    // `\binN` is followed by N raw bytes that may hold braces and
                    // backslashes; they are data, not structure.
                    i = binary_payload_end(self.bytes, i).unwrap_or(i + 2);
                }
                b'{' => {
                    depth += 1;
                    i += 1;
                }
                b'}' => {
                    depth = depth.saturating_sub(1);
                    i += 1;
                    if depth == 0 {
                        return Ok(i);
                    }
                }
                _ => i += 1,
            }
        }
        Err(RtfError::parse(start, "unterminated group"))
    }

    /// The `len` raw bytes at the cursor (the payload of a `\binN` control word, whose
    /// delimiter space the tokeniser has already consumed). The cursor moves past them.
    pub(crate) fn take_binary(&mut self, len: usize) -> Result<&'a [u8], RtfError> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| RtfError::parse(self.pos, "binary data runs past the stream"))?;
        let data = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(data)
    }

    pub(crate) fn next_token(&mut self) -> Result<Option<Token<'a>>, RtfError> {
        loop {
            if self.pos >= self.bytes.len() {
                return Ok(None);
            }
            match self.bytes[self.pos] {
                b'\r' | b'\n' => {
                    self.pos += 1;
                }
                b'{' => {
                    self.pos += 1;
                    return Ok(Some(Token::GroupStart));
                }
                b'}' => {
                    self.pos += 1;
                    return Ok(Some(Token::GroupEnd));
                }
                b'\\' => return self.read_escape().map(Some),
                _ => {
                    let start = self.pos;
                    while self.pos < self.bytes.len()
                        && !matches!(self.bytes[self.pos], b'\\' | b'{' | b'}' | b'\r' | b'\n')
                    {
                        self.pos += 1;
                    }
                    return Ok(Some(Token::Text(&self.bytes[start..self.pos])));
                }
            }
        }
    }

    fn read_escape(&mut self) -> Result<Token<'a>, RtfError> {
        let start = self.pos;
        self.pos += 1; // the backslash
        let Some(&first) = self.bytes.get(self.pos) else {
            return Err(RtfError::parse(start, "a backslash ends the stream"));
        };
        if first.is_ascii_alphabetic() {
            let name_start = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_alphabetic() {
                self.pos += 1;
            }
            let name = std::str::from_utf8(&self.bytes[name_start..self.pos])
                .map_err(|_| RtfError::parse(name_start, "a control word is not ASCII"))?;
            let param = self.read_parameter()?;
            // One space after a control word is its delimiter and is not text.
            if self.bytes.get(self.pos) == Some(&b' ') {
                self.pos += 1;
            }
            return Ok(Token::ControlWord { name, param });
        }
        self.pos += 1;
        if first == b'\'' {
            let hi = self
                .bytes
                .get(self.pos)
                .copied()
                .and_then(hex_value)
                .ok_or_else(|| RtfError::parse(self.pos, "\\' needs two hex digits"))?;
            let lo = self
                .bytes
                .get(self.pos + 1)
                .copied()
                .and_then(hex_value)
                .ok_or_else(|| RtfError::parse(self.pos, "\\' needs two hex digits"))?;
            self.pos += 2;
            return Ok(Token::HexByte((hi << 4) | lo));
        }
        Ok(Token::ControlSymbol(first))
    }

    /// Reads an optional signed parameter after a control word. A `-` that is not followed by
    /// a digit is not a parameter (it belongs to the following text).
    fn read_parameter(&mut self) -> Result<Option<i32>, RtfError> {
        let start = self.pos;
        let mut negative = false;
        if self.bytes.get(self.pos) == Some(&b'-') {
            if !self.bytes.get(self.pos + 1).is_some_and(u8::is_ascii_digit) {
                return Ok(None);
            }
            negative = true;
            self.pos += 1;
        }
        let digits_start = self.pos;
        while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
        }
        if self.pos == digits_start {
            self.pos = start;
            return Ok(None);
        }
        let mut value: i64 = 0;
        for &digit in &self.bytes[digits_start..self.pos] {
            value = value * 10 + i64::from(digit - b'0');
            if value > i64::from(i32::MAX) + 1 {
                return Err(RtfError::parse(
                    start,
                    "control-word parameter is too large",
                ));
            }
        }
        if negative {
            value = -value;
        }
        if value < i64::from(i32::MIN) || value > i64::from(i32::MAX) {
            return Err(RtfError::parse(
                start,
                "control-word parameter is too large",
            ));
        }
        Ok(Some(value as i32))
    }
}

/// If a `\binN` control word starts at `at` (the backslash), the offset one past its N
/// payload bytes, clamped to the stream. `None` for any other escape.
fn binary_payload_end(bytes: &[u8], at: usize) -> Option<usize> {
    let rest = bytes.get(at + 1..)?;
    if !rest.starts_with(b"bin") {
        return None;
    }
    let mut pos = at + 4;
    let digits_start = pos;
    let mut len = 0usize;
    while let Some(digit) = bytes.get(pos).filter(|byte| byte.is_ascii_digit()) {
        len = len
            .saturating_mul(10)
            .saturating_add(usize::from(digit - b'0'));
        pos += 1;
    }
    if pos == digits_start {
        // `\bin` without a count, or a longer control word such as `\binfsxn`.
        return None;
    }
    if bytes.get(pos) == Some(&b' ') {
        pos += 1;
    }
    Some(pos.saturating_add(len).min(bytes.len()))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(input: &str) -> Vec<Token<'_>> {
        let mut tokenizer = Tokenizer::new(input.as_bytes());
        let mut out = Vec::new();
        loop {
            let token = tokenizer.next_token().expect("token");
            match token {
                Some(token) => out.push(token),
                None => break,
            }
        }
        out
    }

    #[test]
    fn tokenises_groups_text_and_control_words() {
        let got = tokens(r"{\rtf1\ansi Hello \b bold\b0}");
        assert_eq!(got[0], Token::GroupStart);
        assert_eq!(
            got[1],
            Token::ControlWord {
                name: "rtf",
                param: Some(1)
            }
        );
        assert_eq!(
            got[2],
            Token::ControlWord {
                name: "ansi",
                param: None
            }
        );
        assert_eq!(got[3], Token::Text(b"Hello "));
        assert_eq!(
            got[4],
            Token::ControlWord {
                name: "b",
                param: None
            }
        );
        assert_eq!(got[5], Token::Text(b"bold"));
        assert_eq!(
            got[6],
            Token::ControlWord {
                name: "b",
                param: Some(0)
            },
            "a digit after a control word is its parameter"
        );
        assert_eq!(got[7], Token::GroupEnd);
    }

    #[test]
    fn reads_signed_parameters_and_consumes_one_delimiter_space() {
        let got = tokens(r"\li-360 \fs24 \u-10179?");
        assert_eq!(
            got[0],
            Token::ControlWord {
                name: "li",
                param: Some(-360)
            }
        );
        assert_eq!(
            got[1],
            Token::ControlWord {
                name: "fs",
                param: Some(24)
            }
        );
        assert_eq!(
            got[2],
            Token::ControlWord {
                name: "u",
                param: Some(-10179)
            }
        );
        assert_eq!(got[3], Token::Text(b"?"));
    }

    #[test]
    fn hex_bytes_and_control_symbols() {
        let got = tokens(r"\'93\{x\}\\ \*");
        assert_eq!(got[0], Token::HexByte(0x93));
        assert_eq!(got[1], Token::ControlSymbol(b'{'));
        assert_eq!(got[2], Token::Text(b"x"));
        assert_eq!(got[3], Token::ControlSymbol(b'}'));
        assert_eq!(got[4], Token::ControlSymbol(b'\\'));
        assert_eq!(got[5], Token::Text(b" "));
        assert_eq!(got[6], Token::ControlSymbol(b'*'));
    }

    #[test]
    fn a_minus_without_digits_stays_text() {
        let got = tokens(r"\b -x");
        assert_eq!(
            got[0],
            Token::ControlWord {
                name: "b",
                param: None
            }
        );
        assert_eq!(got[1], Token::Text(b"-x"));
    }

    #[test]
    fn raw_groups_include_nested_groups_and_escaped_braces() {
        let input = r"{\*\foo {nested} \} still} tail";
        let mut tokenizer = Tokenizer::new(input.as_bytes());
        assert_eq!(
            tokenizer.next_token().expect("token"),
            Some(Token::GroupStart)
        );
        let raw = tokenizer.take_raw_group(0).expect("raw group");
        assert_eq!(raw, br"{\*\foo {nested} \} still}");
        assert_eq!(tokenizer.position(), raw.len());
    }

    #[test]
    fn a_raw_group_steps_over_binary_data() {
        let input = b"{\\pict\\bin4 }{}}tail} after";
        let mut tokenizer = Tokenizer::new(input);
        let raw = tokenizer.take_raw_group(0).expect("raw group");
        assert_eq!(raw, b"{\\pict\\bin4 }{}}tail}");

        let mut tokenizer = Tokenizer::new(b"\\bin3 a}bc");
        assert_eq!(
            tokenizer.next_token().expect("token"),
            Some(Token::ControlWord {
                name: "bin",
                param: Some(3)
            })
        );
        assert_eq!(tokenizer.take_binary(3).expect("payload"), b"a}b");
        assert!(tokenizer.take_binary(9).is_err(), "past the end");
    }

    #[test]
    fn an_unterminated_group_is_an_error() {
        let mut tokenizer = Tokenizer::new(br"{\b text");
        assert_eq!(
            tokenizer.next_token().expect("token"),
            Some(Token::GroupStart)
        );
        assert!(matches!(
            tokenizer.take_raw_group(0),
            Err(RtfError::Parse { .. })
        ));
    }

    #[test]
    fn a_bom_is_skipped() {
        let mut tokenizer = Tokenizer::new(b"\xEF\xBB\xBF{\\rtf1}");
        assert_eq!(
            tokenizer.next_token().expect("token"),
            Some(Token::GroupStart)
        );
    }
}
