//! `libertum-diff`: compare a chapter with the copy that was last posted (AO3 plan, stage B).
//!
//! Model only: no I/O, no UI. The comparison is in two steps:
//!
//! 1. **Paragraphs.** The old and new paragraph texts are aligned with the patience
//!    algorithm, so an unchanged paragraph always lines up with its twin and a run of edited
//!    paragraphs becomes one block of changes.
//! 2. **Words.** Inside each such block the text is split into words, spaces and punctuation
//!    (Unicode word boundaries) and compared again. Where a paragraph boundary lines up on
//!    both sides the block is cut, so an edit to one paragraph is one [`Hunk`] of its own; a
//!    split or a merge keeps its paragraphs together in one hunk.
//!
//! Each hunk carries what the app needs to show it inline ([`ParaMarks`]: inserted byte
//! ranges and deleted text with the offset it sat at) and to undo it ([`Hunk::revert`]).
//!
//! It is never on the keystroke path: the app runs it on request, off the UI thread. Inputs
//! over [`Options::max_bytes`] are refused with [`CompareError::TooLarge`] rather than
//! compared slowly, and the word step stops refining at [`Options::deadline`] (the result is
//! then still correct, only less tidy).
//!
//! Only top-level paragraphs are compared ([`paragraphs`]); tables and the generated table
//! of contents are left out, because a revert could not rebuild their structure from text.

use std::ops::Range;
use std::time::{Duration, Instant};

use libertum_model::{BlockId, Document};
use similar::{Algorithm, DiffOp, capture_diff_slices_deadline};
use unicode_segmentation::UnicodeSegmentation;

/// A paragraph of the current document: its id (so marks and reverts can name it) and text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewParagraph<'a> {
    pub id: BlockId,
    pub text: &'a str,
}

/// Limits for one comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Old and new text together, bytes. Above this the comparison is refused.
    pub max_bytes: usize,
    /// How long the word step may spend refining before it settles for a coarser result.
    pub deadline: Duration,
}

impl Default for Options {
    /// 40 MB of text (several very long books) and two seconds.
    fn default() -> Self {
        Self {
            max_bytes: 40 * 1024 * 1024,
            deadline: Duration::from_secs(2),
        }
    }
}

/// Why a comparison was not made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompareError {
    /// The texts together are `bytes` long, over the `limit`.
    TooLarge { bytes: usize, limit: usize },
}

impl std::fmt::Display for CompareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { bytes, limit } => write!(
                f,
                "the text is too long to compare ({bytes} bytes; the limit is {limit})"
            ),
        }
    }
}

impl std::error::Error for CompareError {}

/// What a hunk did to the posted text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HunkKind {
    /// Paragraphs that were not in the posted copy.
    Added,
    /// Posted paragraphs that are gone.
    Removed,
    /// Posted paragraphs whose text changed (possibly split or merged).
    Changed,
}

/// Text that was deleted from a paragraph, and where in the current paragraph it was.
/// A `'\n'` inside `text` is a paragraph break that was removed (two paragraphs merged).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deletion {
    /// Byte offset in the current paragraph's text, on a character boundary.
    pub offset: usize,
    pub text: String,
}

/// How to draw one current paragraph's changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParaMarks {
    pub block: BlockId,
    /// Inserted text, as sorted, non-overlapping byte ranges of the paragraph.
    pub inserted: Vec<Range<usize>>,
    /// Deleted text, sorted by offset.
    pub deleted: Vec<Deletion>,
}

/// One text replacement inside a paragraph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    /// Byte range of the current text to replace (empty for a pure insertion).
    pub range: Range<usize>,
    pub replacement: String,
}

/// How to put a hunk's posted text back, as editor operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Revert {
    /// Replace text inside one paragraph. The edits are sorted by descending offset, so
    /// applying them in order leaves the offsets of the ones still to come valid. The
    /// paragraph keeps its formatting outside the edited ranges.
    EditText {
        block: BlockId,
        edits: Vec<TextEdit>,
    },
    /// Remove the paragraphs `remove` (in document order) and put paragraphs with `texts`
    /// in their place: after `after` (or, when `after` is `None`, before `before`; when both
    /// are `None` the compared slice is empty). The new paragraphs take body formatting.
    Replace {
        remove: Vec<BlockId>,
        after: Option<BlockId>,
        before: Option<BlockId>,
        texts: Vec<String>,
    },
}

/// One change between the posted copy and the current text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub kind: HunkKind,
    /// Indices into the old (posted) paragraph slice.
    pub old_range: Range<usize>,
    /// The posted paragraphs' texts (empty for [`HunkKind::Added`]).
    pub old_texts: Vec<String>,
    /// Indices into the new paragraph slice.
    pub new_range: Range<usize>,
    /// The current paragraphs' ids (empty for [`HunkKind::Removed`]).
    pub new_ids: Vec<BlockId>,
    /// The current paragraph just before the hunk, if any (where removed text sits).
    pub after: Option<BlockId>,
    /// The current paragraph just after the hunk, if any.
    pub before: Option<BlockId>,
    /// Inline marks, one entry per current paragraph that has any.
    pub marks: Vec<ParaMarks>,
    /// In-place edits when the hunk is one posted paragraph against one current paragraph.
    text_edits: Option<Vec<TextEdit>>,
}

impl Hunk {
    /// The edit that puts the posted text back. One paragraph against one paragraph is
    /// edited in place (its formatting kept); anything else replaces whole paragraphs.
    pub fn revert(&self) -> Revert {
        if let (Some(edits), [block]) = (&self.text_edits, self.new_ids.as_slice()) {
            return Revert::EditText {
                block: *block,
                edits: edits.clone(),
            };
        }
        Revert::Replace {
            remove: self.new_ids.clone(),
            after: self.after,
            before: self.before,
            texts: self.old_texts.clone(),
        }
    }
}

/// The result of a comparison.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Comparison {
    /// In document order.
    pub hunks: Vec<Hunk>,
}

/// Compares the posted paragraphs `old` with the current paragraphs `new`, with the default
/// [`Options`].
pub fn compare<S: AsRef<str>>(
    old: &[S],
    new: &[NewParagraph<'_>],
) -> Result<Comparison, CompareError> {
    compare_with(old, new, &Options::default())
}

/// Compares the posted paragraphs `old` with the current paragraphs `new`. Pass a chapter's
/// slices to compare one chapter.
pub fn compare_with<S: AsRef<str>>(
    old: &[S],
    new: &[NewParagraph<'_>],
    options: &Options,
) -> Result<Comparison, CompareError> {
    let bytes: usize = old.iter().map(|text| text.as_ref().len()).sum::<usize>()
        + new.iter().map(|para| para.text.len()).sum::<usize>();
    if bytes > options.max_bytes {
        return Err(CompareError::TooLarge {
            bytes,
            limit: options.max_bytes,
        });
    }
    let deadline = Instant::now().checked_add(options.deadline);
    let old: Vec<&str> = old.iter().map(AsRef::as_ref).collect();
    let new_texts: Vec<&str> = new.iter().map(|para| para.text).collect();

    let mut hunks = Vec::new();
    let ops = capture_diff_slices_deadline(Algorithm::Patience, &old, &new_texts, deadline);
    for op in ops {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert {
                new_index, new_len, ..
            } => hunks.push(whole(
                HunkKind::Added,
                &old,
                new,
                0..0,
                new_index..new_index + new_len,
            )),
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => hunks.push(whole(
                HunkKind::Removed,
                &old,
                new,
                old_index..old_index + old_len,
                new_index..new_index,
            )),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => words(
                &old,
                new,
                old_index..old_index + old_len,
                new_index..new_index + new_len,
                deadline,
                &mut hunks,
            ),
        }
    }
    Ok(Comparison { hunks })
}

/// The top-level paragraphs among `blocks` (top-level block indices, as
/// `libertum_export::Chapter::start..end`), leaving out tables and table-of-contents entries.
pub fn paragraphs(document: &Document, blocks: Range<usize>) -> Vec<NewParagraph<'_>> {
    let body = document.body();
    let end = blocks.end.min(body.len());
    (blocks.start.min(end)..end)
        .filter_map(|index| body.at(index)?.as_paragraph())
        .filter(|para| para.props.toc_target.is_none())
        .map(|para| NewParagraph {
            id: para.id,
            text: para.text(),
        })
        .collect()
}

/// The texts [`paragraphs`] would compare, owned: what "Mark as posted" stores.
pub fn posted_texts(document: &Document, blocks: Range<usize>) -> Vec<String> {
    paragraphs(document, blocks)
        .into_iter()
        .map(|para| para.text.to_owned())
        .collect()
}

// ------------------------------------------------------------------ building hunks

fn neighbours(
    new: &[NewParagraph<'_>],
    range: &Range<usize>,
) -> (Option<BlockId>, Option<BlockId>) {
    let after = range
        .start
        .checked_sub(1)
        .and_then(|index| new.get(index))
        .map(|para| para.id);
    let before = new.get(range.end).map(|para| para.id);
    (after, before)
}

/// A hunk of whole paragraphs added or removed.
fn whole(
    kind: HunkKind,
    old: &[&str],
    new: &[NewParagraph<'_>],
    old_range: Range<usize>,
    new_range: Range<usize>,
) -> Hunk {
    let (after, before) = neighbours(new, &new_range);
    let marks = new[new_range.clone()]
        .iter()
        .filter(|para| !para.text.is_empty())
        .map(|para| ParaMarks {
            block: para.id,
            inserted: std::iter::once(0..para.text.len()).collect(),
            deleted: Vec::new(),
        })
        .collect();
    Hunk {
        kind,
        old_texts: old[old_range.clone()]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        old_range,
        new_ids: new[new_range.clone()].iter().map(|para| para.id).collect(),
        new_range,
        after,
        before,
        marks,
        text_edits: None,
    }
}

/// A word, a run of spaces or a punctuation mark inside paragraph `para` (relative to the
/// block being compared), or the break between paragraph `para` and the next.
#[derive(Clone, Copy, Debug)]
struct Token<'a> {
    key: Key<'a>,
    para: usize,
    start: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key<'a> {
    Word(&'a str),
    Break,
}

impl Token<'_> {
    fn end(&self) -> usize {
        match self.key {
            Key::Word(word) => self.start + word.len(),
            Key::Break => self.start,
        }
    }
}

fn tokenise<'a>(texts: impl Iterator<Item = &'a str>) -> Vec<Token<'a>> {
    let mut tokens = Vec::new();
    let mut previous_len = 0;
    for (para, text) in texts.enumerate() {
        if para > 0 {
            // A break's offset is the end of the paragraph before it.
            tokens.push(Token {
                key: Key::Break,
                para: para - 1,
                start: previous_len,
            });
        }
        tokens.extend(text.split_word_bound_indices().map(|(start, word)| Token {
            key: Key::Word(word),
            para,
            start,
        }));
        previous_len = text.len();
    }
    tokens
}

/// A non-equal stretch of tokens: old `old` replaced by new `new` (token indices).
struct Edit {
    old: Range<usize>,
    new: Range<usize>,
}

/// Compares a block of edited paragraphs word by word and cuts it into hunks wherever a
/// paragraph break lines up on both sides.
fn words(
    old: &[&str],
    new: &[NewParagraph<'_>],
    old_block: Range<usize>,
    new_block: Range<usize>,
    deadline: Option<Instant>,
    hunks: &mut Vec<Hunk>,
) {
    let old_tokens = tokenise(old[old_block.clone()].iter().copied());
    let new_tokens = tokenise(new[new_block.clone()].iter().map(|para| para.text));
    let old_keys: Vec<Key<'_>> = old_tokens.iter().map(|t| t.key).collect();
    let new_keys: Vec<Key<'_>> = new_tokens.iter().map(|t| t.key).collect();
    let ops = capture_diff_slices_deadline(Algorithm::Myers, &old_keys, &new_keys, deadline);

    let block = Block {
        old,
        new,
        old_block: &old_block,
        new_block: &new_block,
        old_tokens: &old_tokens,
        new_tokens: &new_tokens,
    };
    // The current piece: first old and new paragraph (relative) and its edits.
    let (mut old_first, mut new_first) = (0usize, 0usize);
    let mut edits: Vec<Edit> = Vec::new();
    for op in ops {
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for i in 0..len {
                    let (o, n) = (old_tokens[old_index + i], new_tokens[new_index + i]);
                    if o.key == Key::Break {
                        // The paragraphs up to this break line up: close the piece.
                        if let Some(hunk) =
                            block.piece(old_first..o.para + 1, new_first..n.para + 1, &edits)
                        {
                            hunks.push(hunk);
                        }
                        edits.clear();
                        old_first = o.para + 1;
                        new_first = n.para + 1;
                    }
                }
            }
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => edits.push(Edit {
                old: old_index..old_index + old_len,
                new: new_index..new_index,
            }),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => edits.push(Edit {
                old: old_index..old_index,
                new: new_index..new_index + new_len,
            }),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => edits.push(Edit {
                old: old_index..old_index + old_len,
                new: new_index..new_index + new_len,
            }),
        }
    }
    if let Some(hunk) = block.piece(
        old_first..old_block.len(),
        new_first..new_block.len(),
        &edits,
    ) {
        hunks.push(hunk);
    }
}

struct Block<'b, 'a> {
    old: &'b [&'a str],
    new: &'b [NewParagraph<'a>],
    old_block: &'b Range<usize>,
    new_block: &'b Range<usize>,
    old_tokens: &'b [Token<'a>],
    new_tokens: &'b [Token<'a>],
}

impl Block<'_, '_> {
    /// Where new token `k` begins: (relative paragraph, byte offset). A break or the end of
    /// the block is the end of the paragraph before it.
    fn new_point(&self, k: usize) -> (usize, usize) {
        match self.new_tokens.get(k) {
            Some(token) => (token.para, token.start),
            None => {
                let last = self.new_block.len().saturating_sub(1);
                (last, self.new[self.new_block.start + last].text.len())
            }
        }
    }

    fn old_text(&self, range: Range<usize>) -> String {
        self.old_tokens[range]
            .iter()
            .map(|token| match token.key {
                Key::Word(word) => word,
                Key::Break => "\n",
            })
            .collect()
    }

    /// One hunk for old paragraphs `old_rel` against new paragraphs `new_rel` (relative to
    /// the block), with `edits` between them. `None` when nothing differs.
    fn piece(&self, old_rel: Range<usize>, new_rel: Range<usize>, edits: &[Edit]) -> Option<Hunk> {
        let old_range = self.old_block.start + old_rel.start..self.old_block.start + old_rel.end;
        let new_range = self.new_block.start + new_rel.start..self.new_block.start + new_rel.end;
        let old_texts: Vec<String> = self.old[old_range.clone()]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let new_paras = &self.new[new_range.clone()];
        let unchanged = old_texts.len() == new_paras.len()
            && old_texts.iter().zip(new_paras).all(|(o, n)| o == n.text);
        if unchanged {
            return None;
        }
        let kind = if old_texts.is_empty() {
            HunkKind::Added
        } else if new_paras.is_empty() {
            HunkKind::Removed
        } else {
            HunkKind::Changed
        };

        // Inline marks, gathered per relative paragraph.
        let mut marks: Vec<ParaMarks> = new_paras
            .iter()
            .map(|para| ParaMarks {
                block: para.id,
                inserted: Vec::new(),
                deleted: Vec::new(),
            })
            .collect();
        if !new_paras.is_empty() {
            for edit in edits {
                if !edit.old.is_empty() {
                    let (para, offset) = self.new_point(edit.new.start);
                    if let Some(mark) = marks.get_mut(para.wrapping_sub(new_rel.start)) {
                        mark.deleted.push(Deletion {
                            offset,
                            text: self.old_text(edit.old.clone()),
                        });
                    }
                }
                for token in &self.new_tokens[edit.new.clone()] {
                    if token.key == Key::Break {
                        continue;
                    }
                    if let Some(mark) = marks.get_mut(token.para.wrapping_sub(new_rel.start)) {
                        match mark.inserted.last_mut() {
                            Some(last) if last.end == token.start => last.end = token.end(),
                            _ => mark.inserted.push(token.start..token.end()),
                        }
                    }
                }
            }
        }
        marks.retain(|mark| !mark.inserted.is_empty() || !mark.deleted.is_empty());

        // One paragraph against one: in-place edits, newest offset first.
        let text_edits = (old_texts.len() == 1 && new_paras.len() == 1).then(|| {
            let mut out: Vec<TextEdit> = edits
                .iter()
                .map(|edit| {
                    let range = if edit.new.is_empty() {
                        let (_, offset) = self.new_point(edit.new.start);
                        offset..offset
                    } else {
                        self.new_tokens[edit.new.start].start
                            ..self.new_tokens[edit.new.end - 1].end()
                    };
                    TextEdit {
                        range,
                        replacement: self.old_text(edit.old.clone()),
                    }
                })
                .collect();
            out.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
            out
        });

        let (after, before) = neighbours(self.new, &new_range);
        Some(Hunk {
            kind,
            old_range,
            old_texts,
            new_ids: new_paras.iter().map(|para| para.id).collect(),
            new_range,
            after,
            before,
            marks,
            text_edits,
        })
    }
}

#[cfg(test)]
mod tests;
