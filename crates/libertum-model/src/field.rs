//! Inline date and time fields.
//!
//! A field is a range of a paragraph's text that shows a date, a time or both, plus the
//! recipe it was written with. The text itself is stored in the paragraph, so selection,
//! word count, layout and every format that only understands text behave normally; the
//! descriptor is what lets the native `.lbtd` format keep the field a field and update an
//! *updated* one when the document is next saved.
//!
//! The model stores no clock and no time zone: [`Document::set_saved_at`] is fed the
//! document's last save time by the app, and formatting a timestamp is the editor's job
//! (crates/libertum-editor). That keeps this crate free of dates and of I/O.
//!
//! [`Document::set_saved_at`]: crate::Document::set_saved_at

use std::ops::Range;

/// What a field's date is taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DateTimeSource {
    /// The moment the field was inserted. It never changes (Word's DATE field).
    Inserted,
    /// The document's last save. Every save rewrites the shown text (Word's SAVEDATE field).
    Updated,
}

/// Which parts of a date and time a field shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateTimeContent {
    DateAndTime,
    Date,
    Time,
}

/// The written form of a field, and the date/time parts it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DateTimeFormat {
    /// `12 November 2026`
    DateLong,
    /// `12 Nov 2026`
    DateMedium,
    /// `12/11/2026`
    DateShort,
    /// `2026-11-12`
    DateIso,
    /// `12 November 2026 at 14:35`
    DateTimeLong,
    /// `12 Nov 2026, 14:35`
    DateTimeMedium,
    /// `2026-11-12 14:35`
    DateTimeIso,
    /// `14:35`
    Time24,
    /// `14:35:09`
    Time24Seconds,
    /// `2:35 pm`
    Time12,
    /// `2:35:09 pm`
    Time12Seconds,
}

impl DateTimeFormat {
    /// Every format, grouped as the picker shows them: date and time, date only, time only.
    pub const ALL: [Self; 11] = [
        Self::DateTimeLong,
        Self::DateTimeMedium,
        Self::DateTimeIso,
        Self::DateLong,
        Self::DateMedium,
        Self::DateShort,
        Self::DateIso,
        Self::Time24,
        Self::Time24Seconds,
        Self::Time12,
        Self::Time12Seconds,
    ];

    pub fn content(self) -> DateTimeContent {
        match self {
            Self::DateLong | Self::DateMedium | Self::DateShort | Self::DateIso => {
                DateTimeContent::Date
            }
            Self::Time24 | Self::Time24Seconds | Self::Time12 | Self::Time12Seconds => {
                DateTimeContent::Time
            }
            Self::DateTimeLong | Self::DateTimeMedium | Self::DateTimeIso => {
                DateTimeContent::DateAndTime
            }
        }
    }
}

/// A date or time shown in a paragraph.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DateTimeField {
    /// The bytes of the paragraph's text the field shows.
    pub range: Range<usize>,
    pub source: DateTimeSource,
    pub format: DateTimeFormat,
}

impl DateTimeField {
    pub fn new(range: Range<usize>, source: DateTimeSource, format: DateTimeFormat) -> Self {
        Self {
            range,
            source,
            format,
        }
    }

    /// True when this field must be rewritten when the document is saved.
    pub fn follows_save(&self) -> bool {
        self.source == DateTimeSource::Updated
    }
}
