//! Paragraphs, runs and blocks (PLAN §4.1).
//!
//! A paragraph owns its own text, so an edit touches one paragraph (or a few when splitting
//! or merging), never a whole-document buffer.

use std::ops::Range;

use crate::bookmark::Bookmark;
use crate::field::DateTimeField;
use crate::format::{CharFormat, ParaFormat, StyleId};
use crate::lists::ListItem;
use crate::media::{CrossRef, InlineImage};
use crate::note::{Note, NoteKind};
use crate::ruby::Ruby;

/// Stable identity of a block for the lifetime of a document.
///
/// Ids are allocated by [`Document::alloc_id`](crate::Document::alloc_id) and never change.
/// Layout caches and undo entries are keyed by them.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct BlockId(pub u64);

/// Directly formatted text: a byte range of the paragraph and the formatting set on it
/// (partial; the rest comes from the paragraph style). Ranges lie on character boundaries.
///
/// A paragraph keeps its runs *normalised*: sorted, non-overlapping, non-empty, with a
/// non-empty format, and adjacent runs with equal formats merged. So two paragraphs that look
/// the same compare equal, which keeps "apply, then undo" exactly checkable.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Run {
    pub range: Range<usize>,
    pub format: CharFormat,
}

/// Horizontal alignment of a paragraph's lines (DOCX `w:jc`, ODT `fo:text-align`).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Align {
    #[default]
    Start,
    Centre,
    End,
    Justify,
}

impl Align {
    /// The value of the HTML `align` attribute for this alignment, or `None` for the
    /// default (start), which needs no attribute.
    pub fn html_value(self) -> Option<&'static str> {
        match self {
            Align::Start => None,
            Align::Centre => Some("center"),
            Align::End => Some("right"),
            Align::Justify => Some("justify"),
        }
    }
}

/// Paragraph-level properties (direct formatting; the style sheet arrives with Phase 2).
///
/// The page-flow properties are modelled the way both formats store them, so import and
/// export map one to one:
/// - `page_break_before`: DOCX `w:pageBreakBefore`, ODT `fo:break-before="page"`. Ctrl/Cmd+
///   Enter sets it (DOCX's own Ctrl+Enter writes a `w:br w:type="page"` run, which the
///   importer will turn into this property on the following paragraph).
/// - `section_start`: the paragraph opens a new section on a new page (a DOCX "next page"
///   section break, an ODT master-page change). Section `n` is the `n`-th such paragraph.
/// - `outline_level`: heading level, 1 = chapter (DOCX `w:outlineLvl`, ODT `text:h`).
/// - `toc_target`: this paragraph is a generated table-of-contents entry for that heading
///   (DOCX TOC field result, ODT `text:table-of-content` body). It is read-only to the user.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ParaProps {
    /// Paragraph style (DOCX `w:pStyle`, ODT `text:style-name`).
    pub style: StyleId,
    /// Direct paragraph formatting over the style (DOCX `w:pPr`, ODT automatic paragraph
    /// style): alignment, indents, spacing and the keep options.
    pub format: ParaFormat,
    pub page_break_before: bool,
    pub section_start: bool,
    pub outline_level: Option<u8>,
    pub toc_target: Option<BlockId>,
    /// Bulleted or numbered list item (DOCX `w:numPr`, ODT `text:list-item`).
    pub list: Option<ListItem>,
}

impl Default for ParaProps {
    fn default() -> Self {
        Self {
            style: StyleId::BODY,
            format: ParaFormat::default(),
            page_break_before: false,
            section_start: false,
            outline_level: None,
            toc_target: None,
            list: None,
        }
    }
}

impl ParaProps {
    /// Properties for the second half of a split at `at` of a paragraph `len` bytes long.
    ///
    /// Page-flow markers stay with the first half (otherwise Enter would start two pages or
    /// two sections). Splitting at the very end of a heading starts a body paragraph, as in
    /// Word and Pages: no heading level and no direct formatting. (The editor then applies the
    /// style sheet's "next style", which this type cannot see.)
    pub fn for_split_tail(self, at: usize, len: usize) -> Self {
        let mut tail = Self {
            page_break_before: false,
            section_start: false,
            toc_target: None,
            ..self
        };
        if at >= len && self.outline_level.is_some() {
            tail.outline_level = None;
            tail.format = ParaFormat::default();
            tail.style = StyleId::BODY;
        }
        tail
    }

    pub fn is_heading(&self) -> bool {
        self.outline_level.is_some()
    }
}

/// A paragraph: stable id, text, styled runs and paragraph properties.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Paragraph {
    pub id: BlockId,
    /// Bumped on every edit. Layout caches are keyed by `(id, version)`.
    pub version: u64,
    text: String,
    runs: Vec<Run>,
    pub props: ParaProps,
    /// Footnotes and endnotes anchored in this paragraph, sorted by offset.
    notes: Vec<Note>,
    /// Named anchors in this paragraph, sorted by offset.
    bookmarks: Vec<Bookmark>,
    /// Inline images anchored in this paragraph, sorted by offset.
    images: Vec<InlineImage>,
    /// Cross-references: ranges of this paragraph's text that show a bookmark's text or
    /// page number. Sorted by start, non-overlapping, never empty.
    cross_refs: Vec<CrossRef>,
    /// Date and time fields: ranges of this paragraph's text that show a date, a time or
    /// both. Sorted by start, non-overlapping, never empty.
    #[serde(default)]
    date_fields: Vec<DateTimeField>,
    /// Ruby: readings set beside ranges of this paragraph's text (furigana, pinyin).
    /// Sorted by start, non-overlapping, never empty.
    #[serde(default)]
    ruby: Vec<Ruby>,
}

impl Paragraph {
    pub fn new(id: BlockId, text: impl Into<String>) -> Self {
        Self {
            id,
            version: 0,
            text: text.into(),
            runs: Vec::new(),
            props: ParaProps::default(),
            notes: Vec::new(),
            bookmarks: Vec::new(),
            images: Vec::new(),
            cross_refs: Vec::new(),
            date_fields: Vec::new(),
            ruby: Vec::new(),
        }
    }

    pub fn with_runs(id: BlockId, text: impl Into<String>, runs: Vec<Run>) -> Self {
        let mut p = Self::new(id, text);
        p.runs = normalise_runs(p.text.len(), runs);
        p
    }

    pub fn with_props(mut self, props: ParaProps) -> Self {
        self.props = props;
        self
    }

    /// Replaces the paragraph properties and bumps the version (a layout cache key).
    pub fn set_props(&mut self, props: ParaProps) -> ParaProps {
        self.version += 1;
        std::mem::replace(&mut self.props, props)
    }

    /// All notes attached to this paragraph, in order.
    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    /// Replaces the notes and returns the old list.
    pub fn set_notes(&mut self, mut notes: Vec<Note>) -> Vec<Note> {
        notes.sort_by_key(|note| note.offset);
        self.version += 1;
        std::mem::replace(&mut self.notes, notes)
    }

    /// Every bookmark anchored in this paragraph, sorted by offset.
    pub fn bookmarks(&self) -> &[Bookmark] {
        &self.bookmarks
    }

    /// Replaces the bookmarks and returns the old list. Sorted by offset, then name, so
    /// undo and round-trip comparisons are deterministic.
    pub fn set_bookmarks(&mut self, mut bookmarks: Vec<Bookmark>) -> Vec<Bookmark> {
        bookmarks.sort_by(|a, b| a.offset.cmp(&b.offset).then_with(|| a.name.cmp(&b.name)));
        self.version += 1;
        std::mem::replace(&mut self.bookmarks, bookmarks)
    }

    /// Inline images anchored in this paragraph, sorted by offset.
    pub fn images(&self) -> &[InlineImage] {
        &self.images
    }

    /// Replaces the images and returns the old list. Offsets are clamped to the text and
    /// snapped to character boundaries; sizes are clamped. The sort is stable, so images at
    /// one offset keep their order.
    pub fn set_images(&mut self, mut images: Vec<InlineImage>) -> Vec<InlineImage> {
        for image in &mut images {
            image.offset = floor_char_boundary(&self.text, image.offset);
            image.clamp_size();
        }
        images.sort_by_key(|image| image.offset);
        self.version += 1;
        std::mem::replace(&mut self.images, images)
    }

    /// Shifts this paragraph's id and the references that point at ids by `shift`, and its
    /// list group by `list_shift`. Used when documents are stitched into one book.
    pub fn restamp(&mut self, shift: u64, list_shift: u64) {
        self.id = BlockId(self.id.0 + shift);
        if let Some(target) = self.props.toc_target {
            self.props.toc_target = Some(BlockId(target.0 + shift));
        }
        if let Some(list) = &mut self.props.list {
            list.group = list.group.saturating_add(list_shift);
        }
    }

    /// Replaces the images without counting as an edit: used when a reader fills in image
    /// bytes, because opening a file must not change a paragraph's version.
    pub(crate) fn restore_images(&mut self, mut images: Vec<InlineImage>) -> Vec<InlineImage> {
        for image in &mut images {
            image.offset = floor_char_boundary(&self.text, image.offset);
            image.clamp_size();
        }
        images.sort_by_key(|image| image.offset);
        std::mem::replace(&mut self.images, images)
    }

    /// Cross-references in this paragraph, sorted by start.
    pub fn cross_refs(&self) -> &[CrossRef] {
        &self.cross_refs
    }

    /// Replaces the cross-references and returns the old list. Ranges are clamped to the
    /// text and snapped to character boundaries; empty and overlapping ones are dropped.
    pub fn set_cross_refs(&mut self, cross_refs: Vec<CrossRef>) -> Vec<CrossRef> {
        let cross_refs = normalise_cross_refs(&self.text, cross_refs);
        self.version += 1;
        std::mem::replace(&mut self.cross_refs, cross_refs)
    }

    /// The cross-reference whose text covers byte `offset`, if any.
    pub fn cross_ref_at(&self, offset: usize) -> Option<&CrossRef> {
        self.cross_refs
            .iter()
            .find(|r| r.range.start <= offset && offset < r.range.end)
    }

    /// Date and time fields in this paragraph, sorted by start.
    pub fn date_fields(&self) -> &[DateTimeField] {
        &self.date_fields
    }

    /// Replaces the date fields and returns the old list. Ranges are clamped to the text and
    /// snapped to character boundaries; empty and overlapping ones are dropped.
    pub fn set_date_fields(&mut self, date_fields: Vec<DateTimeField>) -> Vec<DateTimeField> {
        let date_fields = normalise_date_fields(&self.text, date_fields);
        self.version += 1;
        std::mem::replace(&mut self.date_fields, date_fields)
    }

    /// The date field whose text covers byte `offset`, if any.
    pub fn date_field_at(&self, offset: usize) -> Option<&DateTimeField> {
        self.date_fields
            .iter()
            .find(|f| f.range.start <= offset && offset < f.range.end)
    }

    /// Ruby in this paragraph, sorted by start.
    pub fn ruby(&self) -> &[Ruby] {
        &self.ruby
    }

    /// Replaces the ruby and returns the old list. Ranges are clamped to the text and snapped
    /// to character boundaries; empty ranges, readings with no text and overlaps are dropped.
    pub fn set_ruby(&mut self, ruby: Vec<Ruby>) -> Vec<Ruby> {
        let ruby = normalise_ruby(&self.text, ruby);
        self.version += 1;
        std::mem::replace(&mut self.ruby, ruby)
    }

    /// The ruby whose base text covers byte `offset`, if any.
    pub fn ruby_at(&self, offset: usize) -> Option<&Ruby> {
        self.ruby
            .iter()
            .find(|r| r.range.start <= offset && offset < r.range.end)
    }

    /// The first bookmark with `name` (Word names are case-insensitive).
    pub fn bookmark(&self, name: &str) -> Option<&Bookmark> {
        self.bookmarks
            .iter()
            .find(|bookmark| bookmark.name.eq_ignore_ascii_case(name))
    }

    /// The first footnote's text (compatibility for the single-note UI).
    pub fn note(&self) -> Option<&str> {
        self.notes
            .iter()
            .find(|note| note.kind == NoteKind::Footnote)
            .map(|note| note.text.as_str())
    }

    /// Replaces the first footnote and returns its old text.
    pub fn set_note(&mut self, note: Option<String>) -> Option<String> {
        let old = self
            .notes
            .iter()
            .find(|note| note.kind == NoteKind::Footnote)
            .map(|note| note.text.clone());
        self.notes.retain(|note| note.kind != NoteKind::Footnote);
        if let Some(text) = note {
            self.notes.push(Note {
                kind: NoteKind::Footnote,
                offset: 0,
                text,
            });
        }
        self.notes.sort_by_key(|note| note.offset);
        self.version += 1;
        old
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// Length in bytes (offset space used by the model and parley).
    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    #[inline(always)]
    pub fn word_count(&self) -> usize {
        count_words(&self.text)
    }

    /// Structural equality: same id, text, runs and props. The edit `version` is a cache key,
    /// so undo/redo and round-trip tests compare content, not versions.
    ///
    /// Notes, bookmarks, images and cross-references are compared separately (see
    /// [`anchors_eq`](Self::anchors_eq)): an anchor exactly on a paragraph
    /// boundary cannot be attributed to one side after a merge, so including them here would
    /// make a merge followed by its inverse look unequal even though no text changed.
    pub fn content_eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.text == other.text
            && self.runs == other.runs
            && self.props == other.props
    }

    /// True when the notes, bookmarks, images and cross-references match. Round-trip tests
    /// use this beside [`content_eq`](Self::content_eq).
    pub fn anchors_eq(&self, other: &Self) -> bool {
        self.notes == other.notes
            && self.bookmarks == other.bookmarks
            && self.images == other.images
            && self.cross_refs == other.cross_refs
            && self.date_fields == other.date_fields
            && self.ruby == other.ruby
    }

    /// Inserts `text` at byte offset `at`, keeping runs consistent, and bumps the version.
    ///
    /// `at` is snapped down to a character boundary if necessary.
    pub fn insert_text(&mut self, at: usize, text: &str) {
        let at = floor_char_boundary(&self.text, at);
        self.text.insert_str(at, text);
        let n = text.len();
        // New text takes the formatting of the character before it (or, at the start, of
        // the first character), as typing does in Word and Pages.
        let grows =
            |r: &Run| (r.range.start < at && at <= r.range.end) || (at == 0 && r.range.start == 0);
        let mut grown = false;
        for run in &mut self.runs {
            if !grown && grows(run) {
                run.range.end += n;
                grown = true;
            } else if run.range.start >= at {
                run.range.start += n;
                run.range.end += n;
            }
        }
        for note in &mut self.notes {
            if note.offset >= at {
                note.offset += n;
            }
        }
        self.notes.sort_by_key(|note| note.offset);
        for bookmark in &mut self.bookmarks {
            if bookmark.offset >= at {
                bookmark.offset += n;
            }
        }
        self.bookmarks
            .sort_by(|a, b| a.offset.cmp(&b.offset).then_with(|| a.name.cmp(&b.name)));
        // Text typed at an image's offset goes before the image, so the image stays put
        // relative to what follows it.
        for image in &mut self.images {
            if image.offset >= at {
                image.offset += n;
            }
        }
        // A cross-reference grows only when text lands strictly inside it; text typed at
        // either edge stays outside.
        for r in &mut self.cross_refs {
            if at <= r.range.start {
                r.range.start += n;
                r.range.end += n;
            } else if at < r.range.end {
                r.range.end += n;
            }
        }
        grow_or_move_date_fields(&mut self.date_fields, at, n);
        grow_or_move_ruby(&mut self.ruby, at, n);
        self.version += 1;
    }

    /// Inserts `text` at `at` with exactly the formatting `runs` (ranges relative to `text`;
    /// gaps are unformatted). Undoing a deletion uses this to bring the formatting back.
    pub fn insert_text_formatted(&mut self, at: usize, text: &str, runs: &[Run]) {
        let at = floor_char_boundary(&self.text, at);
        self.insert_text(at, text);
        let range = at..at + text.len();
        let mut kept = Vec::with_capacity(self.runs.len() + runs.len());
        for run in std::mem::take(&mut self.runs) {
            kept.extend(cut_out(run, &range));
        }
        kept.extend(runs.iter().map(|r| Run {
            range: r.range.start + at..r.range.end + at,
            format: r.format.clone(),
        }));
        self.runs = normalise_runs(self.text.len(), kept);
    }

    /// Replaces the runs (normalised) and returns the old ones. Bumps the version.
    pub fn set_runs(&mut self, runs: Vec<Run>) -> Vec<Run> {
        self.version += 1;
        let runs = normalise_runs(self.text.len(), runs);
        std::mem::replace(&mut self.runs, runs)
    }

    /// The runs this paragraph would have with `patch` laid over `range` (the properties
    /// `patch` sets win; the rest of each run is kept). Used to build a `SetRuns` operation.
    pub fn runs_with(&self, range: Range<usize>, patch: &CharFormat) -> Vec<Run> {
        let len = self.text.len();
        let start = floor_char_boundary(&self.text, range.start.min(len));
        let end = ceil_char_boundary(&self.text, range.end.min(len).max(start));
        let mut cuts = vec![0, start, end, len];
        for r in &self.runs {
            cuts.push(r.range.start);
            cuts.push(r.range.end);
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut out = Vec::new();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a >= b {
                continue;
            }
            let own = self.format_of(a);
            let format = if a >= start && b <= end {
                own.overlay(patch)
            } else {
                own
            };
            out.push(Run {
                range: a..b,
                format,
            });
        }
        normalise_runs(len, out)
    }

    /// Cuts `range` (clamped, snapped to character boundaries) at the run boundaries inside it.
    fn pieces(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let len = self.text.len();
        let start = floor_char_boundary(&self.text, range.start.min(len));
        let end = ceil_char_boundary(&self.text, range.end.min(len).max(start));
        let mut cuts = vec![start, end];
        for r in &self.runs {
            for at in [r.range.start, r.range.end] {
                if start < at && at < end {
                    cuts.push(at);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        cuts.windows(2).map(|w| w[0]..w[1]).collect()
    }

    /// The direct formatting every character of `range` shares: a property is set only
    /// when all of them carry the same value (unformatted text counts as carrying none).
    /// Empty when `range` is empty.
    pub fn common_format(&self, range: Range<usize>) -> CharFormat {
        let mut pieces = self.pieces(range).into_iter();
        let Some(first) = pieces.next() else {
            return CharFormat::default();
        };
        let mut common = self.format_of(first.start);
        for piece in pieces {
            let f = self.format_of(piece.start);
            macro_rules! keep {
                ($($field:ident),*) => {$(
                    if common.$field != f.$field {
                        common.$field = None;
                    }
                )*};
            }
            keep!(
                family,
                size_pt,
                weight,
                italic,
                underline,
                strikethrough,
                colour,
                highlight,
                vertical,
                letter_spacing_pt,
                ligatures
            );
        }
        common
    }

    /// The runs this paragraph would have with every property `strip` sets removed from the
    /// direct formatting over `range`, so the text there inherits it from its style instead.
    pub fn runs_without(&self, range: Range<usize>, strip: &CharFormat) -> Vec<Run> {
        let len = self.text.len();
        let start = floor_char_boundary(&self.text, range.start.min(len));
        let end = ceil_char_boundary(&self.text, range.end.min(len).max(start));
        let mut cuts = vec![0, start, end, len];
        for r in &self.runs {
            cuts.push(r.range.start);
            cuts.push(r.range.end);
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut out = Vec::new();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a >= b {
                continue;
            }
            let mut format = self.format_of(a);
            if a >= start && b <= end {
                macro_rules! clear {
                    ($($field:ident),*) => {$(
                        if strip.$field.is_some() {
                            format.$field = None;
                        }
                    )*};
                }
                clear!(
                    family,
                    size_pt,
                    weight,
                    italic,
                    underline,
                    strikethrough,
                    colour,
                    highlight,
                    vertical,
                    letter_spacing_pt,
                    ligatures
                );
            }
            out.push(Run {
                range: a..b,
                format,
            });
        }
        normalise_runs(len, out)
    }

    /// Direct formatting of the character starting at byte `offset` (empty if unformatted).
    pub fn format_of(&self, offset: usize) -> CharFormat {
        self.runs
            .iter()
            .find(|r| r.range.start <= offset && offset < r.range.end)
            .map(|r| r.format.clone())
            .unwrap_or_default()
    }

    /// Direct formatting text typed at `offset` would get: that of the character before it,
    /// or of the first character at the very start.
    pub fn format_for_typing_at(&self, offset: usize) -> CharFormat {
        self.format_of(offset.saturating_sub(1))
    }

    /// Removes `range` and returns the removed text. Offsets are snapped to character
    /// boundaries and clamped to the text, so this never panics.
    pub fn delete_range(&mut self, range: Range<usize>) -> String {
        self.delete_range_with_runs(range).0
    }

    /// Like [`delete_range`](Self::delete_range), also returning the formatting of the
    /// removed text (ranges relative to it), so the deletion can be undone exactly.
    pub fn delete_range_with_runs(&mut self, range: Range<usize>) -> (String, Vec<Run>) {
        let start = floor_char_boundary(&self.text, range.start.min(self.text.len()));
        let end = ceil_char_boundary(&self.text, range.end.min(self.text.len()).max(start));
        if start >= end {
            return (String::new(), Vec::new());
        }
        let removed_runs: Vec<Run> = self
            .runs
            .iter()
            .filter(|r| r.range.start < end && r.range.end > start)
            .map(|r| Run {
                range: r.range.start.max(start) - start..r.range.end.min(end) - start,
                format: r.format.clone(),
            })
            .collect();
        let removed = self.text[start..end].to_owned();
        self.text.replace_range(start..end, "");
        let d = end - start;
        let mut kept = Vec::with_capacity(self.runs.len());
        for mut run in std::mem::take(&mut self.runs) {
            if run.range.end <= start {
                kept.push(run);
            } else if run.range.start >= end {
                run.range.start -= d;
                run.range.end -= d;
                kept.push(run);
            } else {
                // Overlaps the deletion: keep the parts on either side. A run that ends inside
                // the deletion keeps its part before it (`..start`); one that ends after it
                // keeps its tail, shifted left by the deleted length.
                let new_start = run.range.start.min(start);
                let new_end = if run.range.end > end {
                    run.range.end - d
                } else {
                    start
                };
                run.range.start = new_start;
                run.range.end = new_end;
                if run.range.start < run.range.end {
                    kept.push(run);
                }
            }
        }
        self.runs = normalise_runs(self.text.len(), kept);
        for note in &mut self.notes {
            if note.offset >= end {
                note.offset -= d;
            } else if note.offset > start {
                note.offset = start;
            }
        }
        for bookmark in &mut self.bookmarks {
            if bookmark.offset >= end {
                bookmark.offset -= d;
            } else if bookmark.offset > start {
                bookmark.offset = start;
            }
        }
        for image in &mut self.images {
            if image.offset >= end {
                image.offset -= d;
            } else if image.offset > start {
                image.offset = start;
            }
        }
        let moved = |x: usize| {
            if x <= start {
                x
            } else if x >= end {
                x - d
            } else {
                start
            }
        };
        for r in &mut self.cross_refs {
            r.range = moved(r.range.start)..moved(r.range.end);
        }
        self.cross_refs.retain(|r| r.range.start < r.range.end);
        for f in &mut self.date_fields {
            f.range = moved(f.range.start)..moved(f.range.end);
        }
        self.date_fields.retain(|f| f.range.start < f.range.end);
        for r in &mut self.ruby {
            r.range = moved(r.range.start)..moved(r.range.end);
        }
        self.ruby.retain(|r| r.range.start < r.range.end);
        self.version += 1;
        (removed, removed_runs)
    }

    /// Splits the paragraph at `at`: the left half stays here, the right half is returned as
    /// a new paragraph with `new_id`. Both versions are bumped. The right half gets
    /// `tail_props`, or [`ParaProps::for_split_tail`] when `None`.
    pub fn split_off(
        &mut self,
        at: usize,
        new_id: BlockId,
        tail_props: Option<ParaProps>,
    ) -> Paragraph {
        let len = self.text.len();
        let at = floor_char_boundary(&self.text, at.min(len));
        let props = tail_props.unwrap_or_else(|| self.props.for_split_tail(at, len));
        let right_text = self.text.split_off(at);
        let mut left = Vec::new();
        let mut right = Vec::new();
        for mut run in std::mem::take(&mut self.runs) {
            if run.range.end <= at {
                left.push(run);
            } else if run.range.start >= at {
                run.range.start -= at;
                run.range.end -= at;
                right.push(run);
            } else {
                let mut tail = run.clone();
                tail.range.start = 0;
                tail.range.end = run.range.end - at;
                run.range.end = at;
                if run.range.start < run.range.end {
                    left.push(run);
                }
                if tail.range.start < tail.range.end {
                    right.push(tail);
                }
            }
        }
        self.runs = normalise_runs(at, left);
        self.version += 1;
        let right_len = right_text.len();
        let right = normalise_runs(right_len, right);
        let (head_notes, tail_notes): (Vec<Note>, Vec<Note>) = std::mem::take(&mut self.notes)
            .into_iter()
            .partition(|note| note.offset < at);
        self.notes = head_notes;
        let tail_notes = tail_notes
            .into_iter()
            .map(|mut note| {
                note.offset = note.offset.saturating_sub(at);
                note
            })
            .collect();
        let (head_bookmarks, tail_bookmarks): (Vec<Bookmark>, Vec<Bookmark>) =
            std::mem::take(&mut self.bookmarks)
                .into_iter()
                .partition(|bookmark| bookmark.offset < at);
        self.bookmarks = head_bookmarks;
        let tail_bookmarks = tail_bookmarks
            .into_iter()
            .map(|mut bookmark| {
                bookmark.offset = bookmark.offset.saturating_sub(at);
                bookmark
            })
            .collect();
        let (head_images, tail_images): (Vec<InlineImage>, Vec<InlineImage>) =
            std::mem::take(&mut self.images)
                .into_iter()
                .partition(|image| image.offset < at);
        self.images = head_images;
        let tail_images = tail_images
            .into_iter()
            .map(|mut image| {
                image.offset = image.offset.saturating_sub(at);
                image
            })
            .collect();
        // A reference cut by the split keeps its first part; the rest becomes plain text.
        let mut head_refs = Vec::new();
        let mut tail_refs = Vec::new();
        for mut r in std::mem::take(&mut self.cross_refs) {
            if r.range.start >= at {
                r.range = r.range.start - at..r.range.end - at;
                tail_refs.push(r);
            } else {
                r.range.end = r.range.end.min(at);
                head_refs.push(r);
            }
        }
        self.cross_refs = head_refs;
        // The same for a date field cut in two.
        let mut head_fields = Vec::new();
        let mut tail_fields = Vec::new();
        for mut f in std::mem::take(&mut self.date_fields) {
            if f.range.start >= at {
                f.range = f.range.start - at..f.range.end - at;
                tail_fields.push(f);
            } else {
                f.range.end = f.range.end.min(at);
                head_fields.push(f);
            }
        }
        self.date_fields = head_fields;
        // And ruby: the part before the split keeps the reading.
        let mut head_ruby = Vec::new();
        let mut tail_ruby = Vec::new();
        for mut r in std::mem::take(&mut self.ruby) {
            if r.range.start >= at {
                r.range = r.range.start - at..r.range.end - at;
                tail_ruby.push(r);
            } else {
                r.range.end = r.range.end.min(at);
                head_ruby.push(r);
            }
        }
        self.ruby = head_ruby;
        Paragraph {
            id: new_id,
            version: 0,
            text: right_text,
            runs: right,
            props,
            notes: tail_notes,
            bookmarks: tail_bookmarks,
            images: tail_images,
            cross_refs: tail_refs,
            date_fields: tail_fields,
            ruby: tail_ruby,
        }
    }

    /// Appends `other` to the end of this paragraph (the merge half of a split).
    pub fn append(&mut self, mut other: Paragraph) {
        let boundary = self.text.len();
        self.text.push_str(&other.text);
        for run in &mut other.runs {
            run.range.start += boundary;
            run.range.end += boundary;
        }
        for note in &mut other.notes {
            note.offset += boundary;
        }
        for bookmark in &mut other.bookmarks {
            bookmark.offset += boundary;
        }
        self.notes.extend(other.notes);
        self.notes.sort_by_key(|note| note.offset);
        self.bookmarks.extend(other.bookmarks);
        self.bookmarks
            .sort_by(|a, b| a.offset.cmp(&b.offset).then_with(|| a.name.cmp(&b.name)));
        for image in &mut other.images {
            image.offset += boundary;
        }
        self.images.extend(other.images);
        for r in &mut other.cross_refs {
            r.range = r.range.start + boundary..r.range.end + boundary;
        }
        self.cross_refs.extend(other.cross_refs);
        for f in &mut other.date_fields {
            f.range = f.range.start + boundary..f.range.end + boundary;
        }
        self.date_fields.extend(other.date_fields);
        for r in &mut other.ruby {
            r.range = r.range.start + boundary..r.range.end + boundary;
        }
        self.ruby.extend(other.ruby);
        self.runs.append(&mut other.runs);
        self.runs = normalise_runs(self.text.len(), std::mem::take(&mut self.runs));
        self.version += 1;
    }
}

/// A top-level document block: a paragraph or a table. (Section breaks, page breaks and
/// generated TOC entries are modelled on paragraphs for now; opaque preserved content joins
/// this enum with the importers.)
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Block {
    Paragraph(Paragraph),
    Table(crate::table::Table),
}

impl Block {
    pub fn id(&self) -> BlockId {
        match self {
            Block::Paragraph(p) => p.id,
            Block::Table(t) => t.id,
        }
    }

    pub fn as_paragraph(&self) -> Option<&Paragraph> {
        match self {
            Block::Paragraph(p) => Some(p),
            Block::Table(_) => None,
        }
    }

    pub fn as_paragraph_mut(&mut self) -> Option<&mut Paragraph> {
        match self {
            Block::Paragraph(p) => Some(p),
            Block::Table(_) => None,
        }
    }

    pub fn as_table(&self) -> Option<&crate::table::Table> {
        match self {
            Block::Paragraph(_) => None,
            Block::Table(t) => Some(t),
        }
    }

    pub fn as_table_mut(&mut self) -> Option<&mut crate::table::Table> {
        match self {
            Block::Paragraph(_) => None,
            Block::Table(t) => Some(t),
        }
    }

    pub fn word_count(&self) -> usize {
        match self {
            Block::Paragraph(p) => p.word_count(),
            Block::Table(t) => t.word_count(),
        }
    }

    /// Content equality (ignores the paragraph edit version).
    pub fn content_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Block::Paragraph(a), Block::Paragraph(b)) => a.content_eq(b),
            (Block::Table(a), Block::Table(b)) => a.content_eq(b),
            _ => false,
        }
    }
}

/// Puts runs in normal form: clamped to `len`, sorted, without empty ranges or empty
/// formats, and with touching runs of equal format merged.
fn normalise_runs(len: usize, mut runs: Vec<Run>) -> Vec<Run> {
    for run in &mut runs {
        run.range.end = run.range.end.min(len);
    }
    runs.retain(|r| r.range.start < r.range.end && !r.format.is_empty());
    runs.sort_by_key(|r| r.range.start);
    let mut out: Vec<Run> = Vec::with_capacity(runs.len());
    for mut run in runs {
        if let Some(last) = out.last_mut() {
            if last.range.end == run.range.start && last.format == run.format {
                last.range.end = run.range.end;
                continue;
            }
            // Overlaps (never produced by this module) keep the earlier run's claim.
            if run.range.start < last.range.end {
                if run.range.end <= last.range.end {
                    continue;
                }
                run.range.start = last.range.end;
            }
        }
        out.push(run);
    }
    out
}

/// Puts cross-references in normal form: clamped to the text, on character boundaries,
/// sorted, non-empty and non-overlapping (an overlapping one is dropped).
/// Keeps date fields inside the text, sorted and non-overlapping.
fn normalise_date_fields(text: &str, mut fields: Vec<DateTimeField>) -> Vec<DateTimeField> {
    let len = text.len();
    for f in &mut fields {
        let start = floor_char_boundary(text, f.range.start.min(len));
        let end = ceil_char_boundary(text, f.range.end.min(len).max(start));
        f.range = start..end;
    }
    fields.retain(|f| f.range.start < f.range.end);
    fields.sort_by_key(|f| f.range.start);
    let mut out: Vec<DateTimeField> = Vec::with_capacity(fields.len());
    for f in fields {
        if let Some(last) = out.last()
            && f.range.start < last.range.end
        {
            continue;
        }
        out.push(f);
    }
    out
}

/// Text typed inside a date field grows it; text at either edge stays outside, as with
/// cross-references.
fn grow_or_move_date_fields(fields: &mut [DateTimeField], at: usize, n: usize) {
    for f in fields {
        if at <= f.range.start {
            f.range.start += n;
            f.range.end += n;
        } else if at < f.range.end {
            f.range.end += n;
        }
    }
}

/// Keeps ruby inside the text, sorted and non-overlapping, with a reading each.
fn normalise_ruby(text: &str, mut ruby: Vec<Ruby>) -> Vec<Ruby> {
    let len = text.len();
    for r in &mut ruby {
        let start = floor_char_boundary(text, r.range.start.min(len));
        let end = ceil_char_boundary(text, r.range.end.min(len).max(start));
        r.range = start..end;
    }
    ruby.retain(|r| r.range.start < r.range.end && !r.text.is_empty());
    ruby.sort_by_key(|r| r.range.start);
    let mut out: Vec<Ruby> = Vec::with_capacity(ruby.len());
    for r in ruby {
        if let Some(last) = out.last()
            && r.range.start < last.range.end
        {
            continue;
        }
        out.push(r);
    }
    out
}

/// Text typed inside ruby's base grows it; text at either edge stays outside.
fn grow_or_move_ruby(ruby: &mut [Ruby], at: usize, n: usize) {
    for r in ruby {
        if at <= r.range.start {
            r.range.start += n;
            r.range.end += n;
        } else if at < r.range.end {
            r.range.end += n;
        }
    }
}

fn normalise_cross_refs(text: &str, mut refs: Vec<CrossRef>) -> Vec<CrossRef> {
    for r in &mut refs {
        let start = floor_char_boundary(text, r.range.start);
        let end = ceil_char_boundary(text, r.range.end.max(start));
        r.range = start..end;
    }
    refs.retain(|r| r.range.start < r.range.end);
    refs.sort_by_key(|r| r.range.start);
    let mut out: Vec<CrossRef> = Vec::with_capacity(refs.len());
    for r in refs {
        if out
            .last()
            .is_some_and(|last| r.range.start < last.range.end)
        {
            continue;
        }
        out.push(r);
    }
    out
}

/// The parts of `run` outside `range`.
fn cut_out(run: Run, range: &Range<usize>) -> Vec<Run> {
    let mut out = Vec::new();
    if run.range.start < range.start {
        out.push(Run {
            range: run.range.start..run.range.end.min(range.start),
            format: run.format.clone(),
        });
    }
    if run.range.end > range.end {
        out.push(Run {
            range: run.range.start.max(range.end)..run.range.end,
            format: run.format,
        });
    }
    out
}

pub fn floor_char_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

pub fn ceil_char_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// Counts the words in `text` the way Word does: words are separated by white space, but each
/// Han ideograph and each kana is a word by itself (Chinese and Japanese have no spaces),
/// while Hangul is counted by its space-separated words like any alphabetic script. CJK
/// punctuation and full-width forms are not words; they separate the words around them.
#[inline(always)]
pub fn count_words(text: &str) -> usize {
    // Word counts are adjusted on every keystroke, so keep prose on the vectorised white-space
    // path. Everything the CJK rule treats specially is U+2000 or above, so bytes below 0xE2
    // cannot start one; Latin, Greek, Cyrillic and the rest then need only the ordinary
    // Unicode white-space rules. The CJK loop is out of line so this hot path stays small.
    if text.as_bytes().iter().copied().max().unwrap_or(0) < 0xE2 {
        return text.split_whitespace().count();
    }
    count_words_with_cjk(text)
}

#[cold]
#[inline(never)]
fn count_words_with_cjk(text: &str) -> usize {
    let mut count = 0;
    let mut in_word = false;
    for c in text.chars() {
        if c.is_whitespace() || is_east_asian_punctuation(c) {
            in_word = false;
        } else if is_ideograph_or_kana(c) {
            count += 1;
            in_word = false;
        } else if !in_word {
            count += 1;
            in_word = true;
        }
    }
    count
}

/// A Han ideograph or a kana: counted one by one, since nothing separates them.
pub fn is_ideograph_or_kana(c: char) -> bool {
    matches!(
        u32::from(c),
        0x3005 | 0x3007
            | 0x3021..=0x3029
            | 0x3038..=0x303B
            | 0x3041..=0x30FF
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFF66..=0xFF9F
            | 0x1B000..=0x1B16F
            | 0x20000..=0x2EBEF
            | 0x2F800..=0x2FA1F
            | 0x30000..=0x323AF
    )
}

/// CJK punctuation, vertical forms and full-width ASCII punctuation (not full-width letters
/// and digits, which are words like any others).
fn is_east_asian_punctuation(c: char) -> bool {
    match u32::from(c) {
        // Ideographic space and punctuation, minus the ideographs and marks that are letters.
        0x3000..=0x3004 | 0x3008..=0x3020 | 0x302A..=0x3037 | 0x303C..=0x303F => true,
        0xFE30..=0xFE4F | 0xFF01..=0xFF0F | 0xFF1A..=0xFF20 | 0xFF3B..=0xFF40 => true,
        0xFF5B..=0xFF65 | 0xFFE0..=0xFFEE => true,
        _ => false,
    }
}
