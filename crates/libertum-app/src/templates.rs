//! Document templates offered on the home screen.
//!
//! A template builds a new model [`Document`] (text, paragraph styles, sections and page
//! setup) plus the layout options that supply the fallback fonts. Page setup lives in the
//! document's sections, so the same template opens on any page size the document later gets.

use std::fs;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use libertum_layout::{LayoutOptions, PageGeometry};
use libertum_model::{
    Align, BOLD, BlockId, CharFormat, Document, LineSpacing, PageNumberPos, PageSetup, PaperSize,
    ParaFormat, ParaProps, Paragraph, Run, Section, StyleId, StyleSheet,
};

use crate::state::Units;
use crate::strings as s;

/// The templates on the home screen, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TemplateId {
    #[default]
    Blank,
    /// Standard manuscript format for submitting a novel or story.
    Manuscript,
    /// A book: title page, table of contents, then chapters.
    Book,
}

/// What a template produces.
pub struct NewDocument {
    pub document: Document,
    pub options: LayoutOptions,
}

/// A template the writer added: a copy of an ODT, DOCX or native `.lbtd` file in the
/// templates folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTemplate {
    /// The display name (the file stem, with a suffix if a name was already taken).
    pub name: String,
    /// The copy in the templates folder.
    pub path: PathBuf,
    pub format: TemplateFormat,
}

/// The formats a document template can come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateFormat {
    Odt,
    Docx,
    Native,
}

impl TemplateFormat {
    /// The format named by a path's extension, if it can be a template.
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "odt" => Some(Self::Odt),
            "docx" => Some(Self::Docx),
            "lbtd" => Some(Self::Native),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Odt => "odt",
            Self::Docx => "docx",
            Self::Native => "lbtd",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Odt => s::TEMPLATE_FORMAT_ODT,
            Self::Docx => s::TEMPLATE_FORMAT_DOCX,
            Self::Native => s::TEMPLATE_FORMAT_NATIVE,
        }
    }
}

/// Where added templates are kept (a `templates` folder in the platform data directory).
#[cfg_attr(test, allow(dead_code))]
pub fn default_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "Libertum", "Libertum").map(|dirs| dirs.data_dir().join("templates"))
}

/// The templates in `dir`, newest name order (case-insensitive). Missing folders and files
/// that are not templates are ignored.
pub fn load(dir: &Path) -> Vec<UserTemplate> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut templates: Vec<UserTemplate> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let format = TemplateFormat::from_path(&path)?;
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some(UserTemplate { name, path, format })
        })
        .collect();
    templates.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    templates
}

/// Copies `source` into `dir` as a template. The caller is expected to have checked that it
/// opens as a document. A name already in use gets a numbered suffix.
pub fn add(dir: &Path, source: &Path) -> Result<UserTemplate, String> {
    let format =
        TemplateFormat::from_path(source).ok_or_else(|| s::TEMPLATE_UNSUPPORTED.to_owned())?;
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;

    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "Template".to_owned());
    let mut target = dir.join(format!("{stem}.{}", format.extension()));
    let mut number = 2;
    while target.exists() {
        target = dir.join(format!("{stem} ({number}).{}", format.extension()));
        number += 1;
    }

    if let Err(error) = fs::copy(source, &target) {
        let _ = fs::remove_file(&target);
        return Err(error.to_string());
    }
    let name = target
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or(stem);
    Ok(UserTemplate {
        name,
        path: target,
        format,
    })
}

/// Deletes the template's copy. The file it was made from is untouched.
pub fn remove(template: &UserTemplate) -> Result<(), String> {
    fs::remove_file(&template.path).map_err(|error| error.to_string())
}

/// Manuscript body size, points.
const MANUSCRIPT_SIZE_PT: f32 = 12.0;

/// Half an inch: the manuscript's first-line indent.
const MANUSCRIPT_INDENT_PT: f32 = 36.0;

/// Book body size; line spacing 1.25 of Georgia's own line height is about 1.4 em, a common
/// trade-book setting (estimate from the font's ascent, descent and line gap).
const BOOK_SIZE_PT: f32 = 12.0;
const BOOK_LINE_SPACING: f32 = 1.25;
/// A book's first-line indent (a quarter inch is usual in print).
const BOOK_INDENT_PT: f32 = 18.0;

impl TemplateId {
    pub const ALL: [TemplateId; 3] = [TemplateId::Blank, TemplateId::Manuscript, TemplateId::Book];

    pub fn name(self) -> &'static str {
        match self {
            TemplateId::Blank => s::TEMPLATE_BLANK,
            TemplateId::Manuscript => s::TEMPLATE_MANUSCRIPT,
            TemplateId::Book => s::TEMPLATE_BOOK,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            TemplateId::Blank => s::TEMPLATE_BLANK_DESC,
            TemplateId::Manuscript => s::TEMPLATE_MANUSCRIPT_DESC,
            TemplateId::Book => s::TEMPLATE_BOOK_DESC,
        }
    }

    /// The template to the left (-1) or right (+1) of this one, clamped at the ends.
    pub fn step(self, delta: i32) -> Self {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0) as i32;
        let j = (i + delta).clamp(0, Self::ALL.len() as i32 - 1) as usize;
        Self::ALL[j]
    }

    /// Builds the new document. `units` picks the paper: inches means US Letter, otherwise A4.
    pub fn build(self, units: Units) -> NewDocument {
        let page = PageSetup {
            paper: match units {
                Units::Inches => PaperSize::Letter,
                Units::Centimetres => PaperSize::A4,
            },
            ..PageSetup::a4()
        };
        match self {
            TemplateId::Blank => {
                let mut document = Document::new();
                document.sections = vec![Section {
                    page,
                    ..Section::default()
                }];
                NewDocument {
                    document,
                    options: LayoutOptions::default(),
                }
            }
            TemplateId::Manuscript => manuscript(page),
            TemplateId::Book => book(page),
        }
    }
}

/// Direct paragraph formatting that centres a paragraph and cancels a first-line indent.
fn centred() -> ParaFormat {
    ParaFormat {
        align: Some(Align::Centre),
        indent_first_pt: Some(0.0),
        ..ParaFormat::default()
    }
}

/// Sets paragraph style `id`'s own paragraph and character formatting.
fn restyle(sheet: &mut StyleSheet, id: StyleId, para: ParaFormat, char: Option<CharFormat>) {
    if let Some(style) = sheet.paragraph.get_mut(id.0 as usize) {
        style.para = style.para.overlay(&para);
        if let Some(char) = char {
            style.char = char;
        }
    }
}

fn serif_fonts(first: &[&str]) -> Vec<String> {
    first
        .iter()
        .chain(
            [
                "Times New Roman",
                "Liberation Serif",
                "Tinos",
                "Nimbus Roman",
                "Times",
                "DejaVu Serif",
                "Noto Serif",
            ]
            .iter(),
        )
        .map(|f| (*f).to_owned())
        .collect()
}

/// A paragraph whose whole text has the direct formatting `format`.
fn formatted(id: u64, text: &str, format: CharFormat, props: ParaProps) -> Paragraph {
    Paragraph::with_runs(
        BlockId(id),
        text,
        vec![Run {
            range: 0..text.len(),
            format,
        }],
    )
    .with_props(props)
}

/// Standard manuscript format: 12 pt Times New Roman (or the closest serif installed),
/// double-spaced, 1-inch margins, no space between paragraphs, a half-inch first-line
/// indent, page numbers top right; the title and byline centred, then the first chapter
/// heading (centred, bold, body size) and an empty body paragraph.
//
// TODO(Phase 5): the running header text (surname / short title) beside the page number.
fn manuscript(page: PageSetup) -> NewDocument {
    let options = LayoutOptions {
        font_preference: serif_fonts(&[]),
        base_size_pt: MANUSCRIPT_SIZE_PT,
        ..LayoutOptions::default()
    };
    let mut styles = StyleSheet::with_defaults("Times New Roman", MANUSCRIPT_SIZE_PT);
    styles.para_defaults.space_after_pt = 0.0;
    styles.para_defaults.line_spacing = LineSpacing::Multiple(2.0);
    restyle(
        &mut styles,
        StyleId::BODY,
        ParaFormat {
            indent_first_pt: Some(MANUSCRIPT_INDENT_PT),
            ..ParaFormat::default()
        },
        None,
    );
    // Title and byline: centred at body size.
    restyle(
        &mut styles,
        StyleId::TITLE,
        ParaFormat {
            space_after_pt: Some(0.0),
            ..centred()
        },
        Some(CharFormat::default()),
    );
    // Manuscripts keep chapter headings at body size, in bold.
    restyle(
        &mut styles,
        StyleId::HEADING_1,
        ParaFormat {
            space_before_pt: Some(0.0),
            ..centred()
        },
        Some(CharFormat {
            weight: Some(BOLD),
            ..CharFormat::default()
        }),
    );
    let styled = |id: u64, text: &str, style: StyleId| {
        Paragraph::new(BlockId(id), text).with_props(ParaProps {
            style,
            outline_level: styles.outline_level(style),
            ..ParaProps::default()
        })
    };
    let mut document = Document::from_paragraphs(vec![
        styled(1, s::MANUSCRIPT_TITLE, StyleId::TITLE),
        styled(2, s::MANUSCRIPT_BYLINE, StyleId::TITLE),
        styled(3, "", StyleId::BODY),
        styled(4, s::MANUSCRIPT_CHAPTER, StyleId::HEADING_1),
        styled(5, "", StyleId::BODY),
    ]);
    document.styles = styles;
    document.sections = vec![Section {
        page,
        page_numbers: Some(PageNumberPos::TOP_RIGHT),
        ..Section::default()
    }];
    NewDocument { document, options }
}

/// Book: page 1 is the title page (title and author centred, about a third of the way
/// down), page 2 the table of contents, and page 3 on the chapters. Pages are numbered at
/// the bottom centre from the first chapter, which is page 1; the title and contents pages
/// carry no number (print convention, section 0 of 2).
fn book(page: PageSetup) -> NewDocument {
    let options = LayoutOptions {
        font_preference: serif_fonts(&["Georgia", "Cambria"]),
        base_size_pt: BOOK_SIZE_PT,
        ..LayoutOptions::default()
    };
    let mut styles = StyleSheet::with_defaults("Georgia", BOOK_SIZE_PT);
    styles.para_defaults.space_after_pt = 0.0;
    styles.para_defaults.line_spacing = LineSpacing::Multiple(BOOK_LINE_SPACING);
    // Justified book text needs hyphenation to avoid wide gaps.
    styles.para_defaults.hyphenate = true;
    // Body text: justified with a first-line indent, as in print.
    restyle(
        &mut styles,
        StyleId::BODY,
        ParaFormat {
            align: Some(Align::Justify),
            indent_first_pt: Some(BOOK_INDENT_PT),
            ..ParaFormat::default()
        },
        None,
    );
    restyle(&mut styles, StyleId::TITLE, centred(), None);
    // Chapter headings: centred, well down the page.
    restyle(
        &mut styles,
        StyleId::HEADING_1,
        ParaFormat {
            space_before_pt: Some(72.0),
            space_after_pt: Some(36.0),
            ..centred()
        },
        None,
    );
    restyle(
        &mut styles,
        StyleId::HEADING_2,
        ParaFormat {
            space_after_pt: Some(24.0),
            ..centred()
        },
        None,
    );
    // Lower the title to about a third of the text block.
    let title_drop = PageGeometry::from_setup(&page).content_height() * 0.3;
    let chapter_id = 5;
    let direct = |style: StyleId, format: ParaFormat| ParaProps {
        style,
        format,
        outline_level: styles.outline_level(style),
        ..ParaProps::default()
    };
    let mut document = Document::from_paragraphs(vec![
        Paragraph::new(BlockId(1), s::BOOK_TITLE).with_props(direct(
            StyleId::TITLE,
            ParaFormat {
                space_before_pt: Some(title_drop),
                space_after_pt: Some(18.0),
                ..ParaFormat::default()
            },
        )),
        formatted(
            2,
            s::BOOK_AUTHOR,
            CharFormat::sized(16.0),
            direct(StyleId::BODY, centred()),
        ),
        Paragraph::new(BlockId(3), s::BOOK_CONTENTS).with_props(ParaProps {
            page_break_before: true,
            ..direct(StyleId::HEADING_2, ParaFormat::default())
        }),
        Paragraph::new(BlockId(4), s::BOOK_CHAPTER).with_props(ParaProps {
            toc_target: Some(BlockId(chapter_id)),
            ..direct(
                StyleId::BODY,
                ParaFormat {
                    align: Some(Align::Start),
                    indent_first_pt: Some(0.0),
                    space_after_pt: Some(8.0),
                    ..ParaFormat::default()
                },
            )
        }),
        Paragraph::new(BlockId(chapter_id), s::BOOK_CHAPTER).with_props(ParaProps {
            section_start: true,
            ..direct(StyleId::HEADING_1, ParaFormat::default())
        }),
        Paragraph::new(BlockId(6), ""),
    ]);
    document.styles = styles;
    document.sections = vec![
        Section {
            page,
            ..Section::default()
        },
        Section {
            page,
            page_numbers: Some(PageNumberPos::BOTTOM_CENTRE),
            first_page_number: Some(1),
            ..Section::default()
        },
    ];
    // Numbering belongs to the whole book and starts with chapter one (the third page), so
    // the title and contents pages carry no number.
    document.set_page_numbering(Some(libertum_model::PageNumbering {
        show: true,
        position: PageNumberPos::BOTTOM_CENTRE,
        hide_first_page: false,
        start_page: 3,
        start_at: 1,
    }));
    NewDocument { document, options }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libertum_layout::LayoutState;

    fn texts(doc: &Document) -> Vec<String> {
        doc.body()
            .iter()
            .filter_map(|b| b.as_paragraph())
            .map(|p| p.text().to_owned())
            .collect()
    }

    fn paragraphs(doc: &Document) -> Vec<&Paragraph> {
        doc.body().iter().filter_map(|b| b.as_paragraph()).collect()
    }

    #[test]
    fn blank_is_one_empty_paragraph_on_locale_paper() {
        let a4 = TemplateId::Blank.build(Units::Centimetres);
        assert_eq!(texts(&a4.document), vec![String::new()]);
        assert_eq!(a4.document.page_setup(), PageSetup::a4());
        assert_eq!(a4.document.sections[0].page_numbers, None);
        let letter = TemplateId::Blank.build(Units::Inches);
        assert_eq!(letter.document.page_setup(), PageSetup::letter());
    }

    #[test]
    fn manuscript_is_double_spaced_12pt_with_numbers_top_right() {
        let m = TemplateId::Manuscript.build(Units::Centimetres);
        assert_eq!(m.options.base_size_pt, 12.0);
        let sheet = &m.document.styles;
        assert_eq!(sheet.para_defaults.line_spacing, LineSpacing::Multiple(2.0));
        assert_eq!(m.options.font_preference[0], "Times New Roman");
        assert_eq!(
            texts(&m.document),
            vec![
                s::MANUSCRIPT_TITLE,
                s::MANUSCRIPT_BYLINE,
                "",
                s::MANUSCRIPT_CHAPTER,
                ""
            ]
        );
        let ps = paragraphs(&m.document);
        let resolved: Vec<_> = ps.iter().map(|p| sheet.resolve_props(&p.props)).collect();
        assert!(resolved.iter().all(|r| r.space_after_pt == 0.0));
        assert_eq!(resolved[0].align, Align::Centre, "title centred");
        assert_eq!(resolved[3].align, Align::Centre, "chapter heading centred");
        assert_eq!(
            resolved[4].indent_first_pt, MANUSCRIPT_INDENT_PT,
            "body indented"
        );
        assert!(sheet.resolve_char(ps[3].props.style).is_bold());
        assert_eq!(sheet.resolve_char(ps[3].props.style).size_pt, 12.0);
        assert_eq!(ps[3].props.outline_level, Some(1));
        assert_eq!(&*m.document.styles.defaults.family, "Times New Roman");
        assert_eq!(
            m.document.sections[0].page_numbers,
            Some(PageNumberPos::TOP_RIGHT)
        );
    }

    #[test]
    fn book_is_title_page_then_contents_then_chapters() {
        let b = TemplateId::Book.build(Units::Centimetres);
        let geometry = PageGeometry::from_setup(&b.document.page_setup());
        let mut state = LayoutState::from_document(&b.document, geometry, b.options.clone());
        state.paginate_all();
        assert_eq!(state.page_count(), 3, "title, contents, chapter one");
        let first_text = |page: usize| {
            state.paragraphs()[state.pages()[page].start.para]
                .text
                .clone()
        };
        assert_eq!(first_text(0), s::BOOK_TITLE);
        assert_eq!(first_text(1), s::BOOK_CONTENTS);
        assert_eq!(first_text(2), s::BOOK_CHAPTER);
        // Numbered from the first chapter, which is page 1; the TOC entry points at it.
        let numbers: Vec<(usize, u32)> = state
            .pages()
            .iter()
            .map(|p| (p.section, p.number))
            .collect();
        assert_eq!(numbers[2], (1, 1));
        assert_eq!(state.page_number_of_block(5), Some(1));
        let ps = paragraphs(&b.document);
        let sheet = &b.document.styles;
        assert_eq!(sheet.resolve_props(&ps[0].props).align, Align::Centre);
        assert_eq!(sheet.resolve_props(&ps[5].props).align, Align::Justify);
        assert_eq!(ps[3].props.toc_target, Some(ps[4].id));
        assert_eq!(ps[4].props.outline_level, Some(1));
        // The title sits about a third of the way down its page.
        let title_y = state.pages()[0].slices[0].y;
        assert!(title_y > geometry.content_height() * 0.25);
    }

    #[test]
    fn book_toc_is_already_in_sync() {
        let b = TemplateId::Book.build(Units::Centimetres);
        let mut editor = libertum_editor::Editor::new(b.document);
        let change = editor.refresh_table_of_contents().expect("refresh");
        assert!(!change.structural, "the template's TOC needs no changes");
    }

    #[test]
    fn step_clamps_at_both_ends() {
        assert_eq!(TemplateId::Blank.step(-1), TemplateId::Blank);
        assert_eq!(TemplateId::Blank.step(1), TemplateId::Manuscript);
        assert_eq!(TemplateId::Manuscript.step(1), TemplateId::Book);
        assert_eq!(TemplateId::Book.step(1), TemplateId::Book);
    }
}

#[cfg(test)]
mod user_template_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn folder(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "libertum-templates-{name}-{}-{stamp}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary folder");
        dir
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    #[test]
    fn only_odt_docx_and_lbtd_can_be_templates() {
        assert_eq!(
            TemplateFormat::from_path(Path::new("a.odt")),
            Some(TemplateFormat::Odt)
        );
        assert_eq!(
            TemplateFormat::from_path(Path::new("a.DOCX")),
            Some(TemplateFormat::Docx)
        );
        assert_eq!(
            TemplateFormat::from_path(Path::new("a.lbtd")),
            Some(TemplateFormat::Native)
        );
        assert_eq!(TemplateFormat::from_path(Path::new("a.md")), None);
        assert_eq!(TemplateFormat::from_path(Path::new("a")), None);
    }

    #[test]
    fn adding_copies_and_load_lists_in_name_order() {
        let dir = folder("add");
        let source = fixture("odt/rich.odt");

        let template = add(&dir, &source).expect("add the ODT");
        assert_eq!(template.format, TemplateFormat::Odt);
        assert_eq!(template.name, "rich");
        assert!(template.path.exists());
        assert_ne!(template.path, source, "the template is a copy");

        let second = add(&dir, &source).expect("add the ODT again");
        assert_eq!(second.name, "rich (2)", "a name in use gets a suffix");

        let list = load(&dir);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "rich");
        assert_eq!(list[1].name, "rich (2)");

        remove(&second).expect("remove the second copy");
        assert_eq!(load(&dir).len(), 1);
        assert!(source.exists(), "the source document is untouched");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_files_are_ignored_and_unsupported_files_are_refused() {
        let dir = folder("ignore");
        fs::write(dir.join("notes.txt"), b"not a template").expect("write");
        assert!(load(&dir).is_empty());

        let error = add(&dir, &dir.join("notes.txt")).expect_err("refuse the text file");
        assert!(
            error.contains("ODT"),
            "the error explains the formats: {error}"
        );
        assert!(!dir.join("notes.txt").is_file() || fs::read(dir.join("notes.txt")).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }
}
