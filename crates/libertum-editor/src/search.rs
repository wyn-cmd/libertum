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

use libertum_model::{BlockId, ChangeSet, EditKind, Operation, Paragraph, Run};

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
        let paragraphs: Vec<&Paragraph> = self.doc.paragraphs().collect();
        let start = paragraphs.iter().position(|p| p.id == from.block)?;
        let count = paragraphs.len();
        // One extra pass revisits the starting paragraph unrestricted: that is the wrap.
        for step in 0..=count {
            let index = if forward {
                (start + step) % count
            } else {
                (start + count - step) % count
            };
            let paragraph = paragraphs[index];
            if paragraph.props.toc_target.is_some() {
                continue;
            }
            let text = paragraph.text();
            // The starting paragraph is limited only on the first visit; when the search
            // wraps back to it, everything in it is a candidate again (that is what makes
            // "find" wrap).
            let first_visit = step == 0;
            let found = if forward {
                let limit = if index == start && first_visit {
                    from.offset.min(text.len())
                } else {
                    0
                };
                matches_in(text, needle, options)
                    .into_iter()
                    .find(|range| range.start >= limit)
            } else {
                let limit = if index == start && first_visit {
                    from.offset
                } else {
                    text.len()
                };
                matches_in(text, needle, options)
                    .into_iter()
                    .rfind(|range| range.end <= limit)
            };
            if let Some(range) = found {
                return Some(SearchMatch {
                    block: paragraph.id,
                    range,
                });
            }
        }
        None
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

/// Byte ranges where `needle` occurs in `text`, in order.
pub fn matches_in(text: &str, needle: &str, options: SearchOptions) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if needle.is_empty() || text.is_empty() {
        return out;
    }
    let (haystack, map) = if options.match_case {
        (text.to_owned(), (0..=text.len()).collect::<Vec<_>>())
    } else {
        fold_case(text)
    };
    let needle = if options.match_case {
        needle.to_owned()
    } else {
        needle.to_lowercase()
    };
    let mut at = 0usize;
    while let Some(found) = haystack[at..].find(&needle) {
        let start = at + found;
        let end = start + needle.len();
        // Folded text maps back onto the original's char boundaries.
        let (Some(&start), Some(&end)) = (map.get(start), map.get(end).or(Some(&text.len())))
        else {
            break;
        };
        if !options.whole_word || is_whole_word(text, start, end) {
            out.push(start..end);
        }
        // Advance by one character so overlapping candidates are still considered.
        let next = haystack[start..]
            .char_indices()
            .nth(1)
            .map(|(offset, _)| start + offset);
        match next {
            Some(next) if next > at => at = next,
            _ => break,
        }
    }
    out
}

/// The text lowercased, with a map from each folded byte offset back to the original's.
fn fold_case(text: &str) -> (String, Vec<usize>) {
    let mut folded = String::with_capacity(text.len());
    let mut map = Vec::with_capacity(text.len() + 1);
    for (index, ch) in text.char_indices() {
        for lower in ch.to_lowercase() {
            let len = lower.len_utf8();
            for _ in 0..len {
                map.push(index);
            }
            folded.push(lower);
        }
    }
    map.push(text.len());
    (folded, map)
}

fn is_whole_word(text: &str, start: usize, end: usize) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.is_some_and(word) && !after.is_some_and(word)
}
