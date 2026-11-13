//! The OPC package: every zip entry, kept in its original order (PLAN §6.1).
//!
//! Parts the importer does not understand are never dropped: they stay in this list and are
//! written back on save, byte for byte. Only the parts Libertum rewrites (`document.xml`,
//! `styles.xml`, `numbering.xml`, footnotes and endnotes) are replaced; image parts are only
//! ever added.

use std::io::{Cursor, Read, Write};

use crate::DocxError;

/// One relationship of `word/document.xml`.
#[derive(Clone, Debug)]
pub(crate) struct Relationship {
    pub id: String,
    /// The relationship type URI.
    pub kind: String,
    /// The package part it points at; `None` for an external target (a linked file or URL).
    pub part: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Package {
    entries: Vec<(String, Vec<u8>)>,
}

impl Package {
    pub fn read(bytes: &[u8]) -> Result<Self, DocxError> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        let names: Vec<String> = archive
            .file_names()
            .filter_map(|name| name.ok().map(|name| name.into_owned()))
            .filter(|name| !name.ends_with('/'))
            .collect();
        let mut entries = Vec::with_capacity(names.len());
        for name in names {
            let mut file = archive.by_name(&name)?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            entries.push((name, buf));
        }
        Ok(Self { entries })
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, bytes)| bytes.as_slice())
    }

    /// The UTF-8 text of a part, with a byte-order mark removed.
    pub fn text(&self, name: &str) -> Option<String> {
        let bytes = self.get(name)?;
        let text = String::from_utf8_lossy(bytes);
        Some(text.trim_start_matches('\u{feff}').to_owned())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// The part a relationship from `document.xml` of the given type points at, when that part
    /// is in the package, falling back to `default` when there is no such relationship.
    ///
    /// Word normally uses `word/footnotes.xml` and `word/endnotes.xml`, but the name is only a
    /// convention; reading and writing it back by relationship keeps unusual packages intact.
    pub fn related_part(&self, type_suffix: &str, default: &str) -> Option<String> {
        if let Some(rels) = self.text("word/_rels/document.xml.rels")
            && let Ok(root) = crate::xml::parse(&rels, "word/_rels/document.xml.rels")
        {
            for relationship in root.children_named("Relationship") {
                let matches = relationship
                    .attr("Type")
                    .is_some_and(|relationship_type| relationship_type.ends_with(type_suffix));
                if !matches {
                    continue;
                }
                if let Some(target) = relationship.attr("Target") {
                    let part = resolve_target(target);
                    if self.contains(&part) {
                        return Some(part);
                    }
                }
            }
        }
        self.contains(default).then(|| default.to_owned())
    }

    /// Every relationship of `word/document.xml`, in file order. An unreadable relationships
    /// part gives an empty list.
    pub fn document_relationships(&self) -> Vec<Relationship> {
        const PART: &str = "word/_rels/document.xml.rels";
        let Some(rels) = self.text(PART) else {
            return Vec::new();
        };
        let Ok(root) = crate::xml::parse(&rels, PART) else {
            return Vec::new();
        };
        root.children_named("Relationship")
            .filter_map(|relationship| {
                let external = relationship
                    .attr("TargetMode")
                    .is_some_and(|mode| mode.eq_ignore_ascii_case("External"));
                Some(Relationship {
                    id: relationship.attr("Id")?.to_owned(),
                    kind: relationship.attr("Type").unwrap_or_default().to_owned(),
                    part: match external {
                        true => None,
                        false => relationship.attr("Target").map(resolve_target),
                    },
                })
            })
            .collect()
    }

    pub fn set(&mut self, name: &str, bytes: Vec<u8>) {
        match self.entries.iter_mut().find(|(entry, _)| entry == name) {
            Some(entry) => entry.1 = bytes,
            None => self.entries.push((name.to_owned(), bytes)),
        }
    }

    /// Drops every entry whose name starts with `prefix`.
    pub fn remove_prefix(&mut self, prefix: &str) {
        self.entries.retain(|(name, _)| !name.starts_with(prefix));
    }

    pub fn set_text(&mut self, name: &str, text: String) {
        self.set(name, text.into_bytes());
    }

    pub fn write(&self) -> Result<Vec<u8>, DocxError> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, bytes) in &self.entries {
                writer.start_file(name.as_str(), options)?;
                writer.write_all(bytes)?;
            }
            writer.finish()?;
        }
        Ok(buffer.into_inner())
    }
}

/// Resolves a relationship target against the document part's directory (`word/`).
fn resolve_target(target: &str) -> String {
    let target = target.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    if !target.starts_with('/') {
        parts.push("word");
    }
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}
