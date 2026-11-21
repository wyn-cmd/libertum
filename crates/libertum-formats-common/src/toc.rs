//! What the readers and the app agree on about tables of contents.
//!
//! Word, LibreOffice and Libertum all keep a table of contents as generated content: a list of
//! entries that the program can rebuild from the headings. Libertum models it the same way (a
//! run of entry paragraphs it regenerates), so a reader does not copy a foreign TOC's entries:
//! it marks where the contents go and lets Libertum rebuild them. When it does that to a TOC
//! some other program wrote, the look may change, so the reader raises [`TOC_REBUILT_WARNING`]
//! and the app tells the writer once, with an OK button.

/// The import warning both document readers push when they rebuilt a table of contents that
/// another program wrote. The app matches on this exact text.
pub const TOC_REBUILT_WARNING: &str = "table of contents rebuilt from the headings";

/// Marks a table of contents Libertum wrote itself: the `w:tag` of its DOCX content control and
/// the name of its ODT index. Seeing it on the way back in means the TOC round-trips exactly,
/// so there is nothing to warn about.
pub const LIBERTUM_TOC_TAG: &str = "libertum-toc";
