//! Find and replace (PLAN Phase 8).
//!
//! Search runs over paragraph text only, in body order (table cells included, generated table
//! of contents entries excluded because they are read-only). A match is a byte range inside
//! one paragraph, which is exactly what a selection is, so the app can show a match by
//! selecting it — no separate highlight state, no layout change.
//!
//! `replace_all` applies every replacement in one undo step: the delete and insert
//! operations are gathered into a single [`EditBuilder`] transaction, paragraph by paragraph
//! from the end, so earlier byte offsets stay valid.

use std::ops::Range;
use std::time::Instant;

use libertum_model::{BlockId, ChangeSet, EditKind, Operation, Run};

use super::{DocPosition, EditBuilder, Editor, EditorError, Selection};

/// How a search compares text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    /// Match the query's capitals exactly.
    pub match_case: bool,
    /// Only whole words: the characters either side must not be letters, digits or `_`.
    pub whole_word: bool,
}

/// One occurrence of the query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub block: BlockId,
    /// Byte range in the paragraph's text.
    pub range: Range<usize>,
}

impl Editor {
    /// Finds the next occurrence at or after `from`, wrapping around the document. With
    /// `forward` false it finds the last occurrence before `from`.
    ///
    /// The returned range never starts before `from` when searching forward, so "find next"
    /// from a selection's end moves on rather than finding the same text again.
    pub fn find(
        &self,
        needle: &str,
        options: SearchOptions,
        from: DocPosition,
        forward: bool,
    ) -> Option<SearchMatch> {
        if needle.is_empty() {
            return None;
        }
        let at = self.doc.index_of(from.block)?;
        self.with_matches(needle, options, |matches| {
            if forward {
                // The first match at or after `from`; none left means wrap to the first.
                let next = matches
                    .partition_point(|(index, m)| (*index, m.range.start) < (at, from.offset));
                matches.get(next).or_else(|| matches.first())
            } else {
                // The last match that ends at or before `from`. Matches are ordered by where
                // they start, and overlapping ones can end past `from`, so step back over
                // those.
                let mut before = matches
                    .partition_point(|(index, m)| (*index, m.range.start) < (at, from.offset));
                let found = loop {
                    if before == 0 {
                        break None;
                    }
                    before -= 1;
                    let (index, m) = &matches[before];
                    if *index < at || m.range.end <= from.offset {
                        break Some(&matches[before]);
                    }
                };
                found.or_else(|| matches.last())
            }
            .map(|(_, m)| m.clone())
        })
    }

    /// Runs `use_matches` on every match of `needle` in the document, in order, each with
    /// its paragraph's position in the document. The list is built once per query and
    /// document revision and reused until either changes, so stepping through matches and
    /// counting them does not read the text again.
    fn with_matches<R>(
        &self,
        needle: &str,
        options: SearchOptions,
        use_matches: impl FnOnce(&[(usize, SearchMatch)]) -> R,
    ) -> R {
        let mut cache = self.search_cache.lock().unwrap_or_else(|e| e.into_inner());
        let revision = self.doc.revision();
        let fresh = cache
            .as_ref()
            .is_some_and(|c| c.revision == revision && c.options == options && c.needle == needle);
        if !fresh {
            let mut searcher = Searcher::new(needle, options);
            let mut matches = Vec::new();
            let mut ranges = Vec::new();
            let mut scan = |index: usize, paragraph: &libertum_model::Paragraph| {
                // Generated table of contents entries are read-only: never a match.
                if paragraph.props.toc_target.is_some() {
                    return;
                }
                ranges.clear();
                searcher.search(paragraph.text(), &mut ranges);
                matches.extend(ranges.drain(..).map(|range| {
                    (
                        index,
                        SearchMatch {
                            block: paragraph.id,
                            range,
                        },
                    )
                }));
            };
            // Typing extends the query a character at a time. A longer query can only match
            // in paragraphs the shorter one matched in, so only those are read again.
            // (Not for whole words: "cat" as a word does not occur where "cats" does.)
            let narrowed = cache.as_ref().filter(|c| {
                c.revision == revision
                    && c.options == options
                    && !options.whole_word
                    && !c.needle.is_empty()
                    && Searcher::new(needle, options)
                        .needle
                        .starts_with(&Searcher::new(&c.needle, options).needle)
            });
            match narrowed {
                Some(previous) => {
                    let mut last = None;
                    for (index, _) in &previous.matches {
                        if last == Some(*index) {
                            continue;
                        }
                        last = Some(*index);
                        if let Some(paragraph) = self.doc.paragraph_at(*index) {
                            scan(*index, paragraph);
                        }
                    }
                }
                None => {
                    for (index, paragraph) in self.doc.paragraphs().enumerate() {
                        scan(index, paragraph);
                    }
                }
            }
            *cache = Some(MatchCache {
                needle: needle.to_owned(),
                options,
                revision,
                matches,
            });
        }
        use_matches(cache.as_ref().map_or(&[], |c| c.matches.as_slice()))
    }

    /// How many times `needle` occurs in the document, and which occurrence `current` is
    /// (1-based, in document order), if it is one of them. Table-of-contents entries are
    /// skipped, as in [`find`](Self::find). The match list is shared with `find`, so after
    /// a search this is a lookup, not another pass over the text.
    pub fn match_position(
        &self,
        needle: &str,
        options: SearchOptions,
        current: Option<&SearchMatch>,
    ) -> (usize, Option<usize>) {
        if needle.is_empty() {
            return (0, None);
        }
        let at = current.and_then(|c| self.doc.index_of(c.block).map(|index| (index, c)));
        self.with_matches(needle, options, |matches| {
            let position = at.and_then(|(index, current)| {
                let first = matches
                    .partition_point(|(i, m)| (*i, m.range.start) < (index, current.range.start));
                matches[first..]
                    .iter()
                    .take_while(|(i, m)| *i == index && m.range.start == current.range.start)
                    .position(|(_, m)| m.range == current.range)
                    .map(|offset| first + offset + 1)
            });
            (matches.len(), position)
        })
    }

    /// Replaces one match with `replacement` (one undo step). The new text takes the
    /// formatting of the text it replaces.
    pub fn replace_match(
        &mut self,
        found: &SearchMatch,
        replacement: &str,
    ) -> Result<ChangeSet, EditorError> {
        let Some(paragraph) = self.doc.paragraph(found.block) else {
            return Ok(ChangeSet::default());
        };
        if found.range.start > found.range.end || found.range.end > paragraph.len() {
            return Ok(ChangeSet::default());
        }
        // The replacement takes the formatting the replaced text started with, and the caret
        // lands after it.
        let format = vec![Run {
            range: 0..replacement.len(),
            format: paragraph.format_of(found.range.start),
        }];
        let mut b = EditBuilder::default();
        b.apply(
            &mut self.doc,
            Operation::DeleteRange {
                block: found.block,
                range: found.range.clone(),
            },
        )?;
        if !replacement.is_empty() {
            b.apply(
                &mut self.doc,
                Operation::InsertText {
                    block: found.block,
                    offset: found.range.start,
                    text: replacement.to_owned(),
                    format: Some(format),
                },
            )?;
        }
        let dirty = b.dirty.clone();
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        let caret = DocPosition {
            block: found.block,
            offset: found.range.start + replacement.len(),
        };
        self.selection = Selection::caret(self.clamp(caret));
        self.preferred_x = None;
        Ok(dirty)
    }

    /// Replaces every occurrence in the document and returns how many were replaced. One
    /// undo step for the whole document.
    pub fn replace_all(
        &mut self,
        needle: &str,
        replacement: &str,
        options: SearchOptions,
    ) -> Result<(ChangeSet, usize), EditorError> {
        if needle.is_empty() {
            return Ok((ChangeSet::default(), 0));
        }
        // Collect first: the plan is read-only, so the edits below cannot disturb it.
        let mut plan: Vec<(BlockId, Vec<Range<usize>>)> = Vec::new();
        let mut total = 0usize;
        for paragraph in self.doc.paragraphs() {
            if paragraph.props.toc_target.is_some() {
                continue;
            }
            let ranges = matches_in(paragraph.text(), needle, options);
            if !ranges.is_empty() {
                total += ranges.len();
                plan.push((paragraph.id, ranges));
            }
        }
        if total == 0 {
            return Ok((ChangeSet::default(), 0));
        }
        let mut b = EditBuilder::default();
        for (block, ranges) in &plan {
            // From the end, so the earlier offsets in this paragraph stay valid.
            for range in ranges.iter().rev() {
                // `InsertText` wants runs relative to the inserted text: the replacement
                // takes the formatting the replaced text started with.
                let format = self.doc.paragraph(*block).map(|p| {
                    vec![Run {
                        range: 0..replacement.len(),
                        format: p.format_of(range.start),
                    }]
                });
                b.apply(
                    &mut self.doc,
                    Operation::DeleteRange {
                        block: *block,
                        range: range.clone(),
                    },
                )?;
                if !replacement.is_empty() {
                    b.apply(
                        &mut self.doc,
                        Operation::InsertText {
                            block: *block,
                            offset: range.start,
                            text: replacement.to_owned(),
                            format,
                        },
                    )?;
                }
            }
        }
        let dirty = b.dirty.clone();
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok((dirty, total))
    }
}

/// Every match of one query in one revision of a document, in document order, each with
/// the index of its paragraph in the document.
pub(crate) struct MatchCache {
    needle: String,
    options: SearchOptions,
    revision: u64,
    matches: Vec<(usize, SearchMatch)>,
}

/// Byte ranges where `needle` occurs in `text`, in order.
pub fn matches_in(text: &str, needle: &str, options: SearchOptions) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    Searcher::new(needle, options).search(text, &mut out);
    out
}

/// A query prepared once and run over many paragraphs. It keeps its scratch buffers between
/// paragraphs, so a document-wide search allocates per query, not per paragraph.
struct Searcher {
    /// The query, lowercased unless the search matches case.
    needle: String,
    options: SearchOptions,
    /// The paragraph lowercased (case-insensitive searches only).
    folded: String,
    /// Folded byte offset to original byte offset, filled only for text whose lowercase
    /// form has a different byte layout (for example U+0130, capital I with a dot).
    map: Vec<usize>,
}

impl Searcher {
    fn new(needle: &str, options: SearchOptions) -> Self {
        Self {
            needle: if options.match_case {
                needle.to_owned()
            } else {
                needle.to_lowercase()
            },
            options,
            folded: String::new(),
            map: Vec::new(),
        }
    }

    /// Appends the byte ranges of `text` that match to `out`, in order. Overlapping
    /// occurrences are all reported.
    fn search(&mut self, text: &str, out: &mut Vec<Range<usize>>) {
        if self.needle.is_empty() || text.is_empty() {
            return;
        }
        // What is searched, and whether its offsets are the original's.
        let mut mapped = false;
        let haystack: &str = if self.options.match_case {
            text
        } else if text.is_ascii() {
            // The common case: lowercase in place, same offsets.
            self.folded.clear();
            self.folded.push_str(text);
            self.folded.make_ascii_lowercase();
            &self.folded
        } else {
            self.folded.clear();
            self.map.clear();
            // Offsets stay the original's as long as every character lowercases to one
            // character of the same length; the map is only built from the first
            // character that does not.
            let bytes = text.as_bytes();
            let mut index = 0usize;
            while index < bytes.len() {
                // A run of ASCII is copied and lowercased in one go.
                let run = bytes[index..]
                    .iter()
                    .position(|b| !b.is_ascii())
                    .unwrap_or(bytes.len() - index);
                if run > 0 {
                    let before = self.folded.len();
                    self.folded.push_str(&text[index..index + run]);
                    self.folded[before..].make_ascii_lowercase();
                    if mapped {
                        self.map.extend(index..index + run);
                    }
                    index += run;
                    continue;
                }
                let Some(ch) = text[index..].chars().next() else {
                    break;
                };
                let mut lower = ch.to_lowercase();
                let first = lower.next().unwrap_or(ch);
                let single = lower.len() == 0 && first.len_utf8() == ch.len_utf8();
                if !mapped && !single {
                    mapped = true;
                    // Everything folded so far kept its offsets.
                    self.map.extend(0..self.folded.len());
                }
                let before = self.folded.len();
                self.folded.push(first);
                self.folded.extend(lower);
                if mapped {
                    let added = self.folded.len() - before;
                    self.map.extend(std::iter::repeat_n(index, added));
                }
                index += ch.len_utf8();
            }
            if mapped {
                self.map.push(text.len());
            }
            &self.folded
        };
        let needle = self.needle.as_str();
        let mut at = 0usize;
        while let Some(found) = haystack.get(at..).and_then(|rest| rest.find(needle)) {
            let start = at + found;
            let end = start + needle.len();
            let (original_start, original_end) = if mapped {
                match (self.map.get(start), self.map.get(end)) {
                    (Some(&s), Some(&e)) => (s, e),
                    _ => break,
                }
            } else {
                (start, end)
            };
            // A folded match can end inside the expansion of one original character; such
            // a range is empty or not on a boundary, and is not a match of whole characters.
            let valid = original_start < original_end
                && text.is_char_boundary(original_start)
                && text.is_char_boundary(original_end);
            if valid
                && (!self.options.whole_word || is_whole_word(text, original_start, original_end))
            {
                out.push(original_start..original_end);
            }
            // Advance by one character so overlapping candidates are still considered.
            let step = haystack[start..].chars().next().map_or(1, char::len_utf8);
            at = start + step;
        }
    }
}

fn is_whole_word(text: &str, start: usize, end: usize) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.is_some_and(word) && !after.is_some_and(word)
}
