//! The ODF package: every zip entry, kept in its original order (PLAN §6.1, §6.3).
//!
//! Parts the importer does not understand are never dropped: they stay in this list and are
//! written back on save, byte for byte. Only `content.xml` and `styles.xml` are replaced.
//!
//! ODF packaging adds one hard rule: the `mimetype` entry is the first entry in the zip and
//! is stored without compression. [`Package::write_odf`] enforces it on every save, even for
//! a package that arrived with the entry compressed or in the wrong place.

use std::io::{Cursor, Read, Write};

use crate::{MIME_TEXT, OdtError};

#[derive(Clone, Debug, Default)]
pub(crate) struct Package {
    entries: Vec<(String, Vec<u8>)>,
}

impl Package {
    pub fn read(bytes: &[u8]) -> Result<Self, OdtError> {
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

    /// Every part name, in package order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(name, _)| name.as_str())
    }

    pub fn set(&mut self, name: &str, bytes: Vec<u8>) {
        match self.entries.iter_mut().find(|(entry, _)| entry == name) {
            Some(entry) => entry.1 = bytes,
            None => self.entries.push((name.to_owned(), bytes)),
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.entries.retain(|(entry, _)| entry != name);
    }

    pub fn set_text(&mut self, name: &str, text: String) {
        self.set(name, text.into_bytes());
    }

    /// Writes the package with ODF's packaging rules applied.
    pub fn write_odf(&self) -> Result<Vec<u8>, OdtError> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let stored = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer.start_file("mimetype", stored)?;
            writer.write_all(MIME_TEXT.as_bytes())?;
            let deflated = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, bytes) in &self.entries {
                if name == "mimetype" {
                    continue;
                }
                writer.start_file(name.as_str(), deflated)?;
                writer.write_all(bytes)?;
            }
            writer.finish()?;
        }
        Ok(buffer.into_inner())
    }
}
