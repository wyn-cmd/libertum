//! Tags hidden for one work: the ones the writer already copied (they are on the work now)
//! and the ones they removed with the – button as unrelated. Neither is suggested again in
//! any chapter of that document until Show Again clears the list. Kept beside the document
//! in its history folder (`ao3-tags.json`); a document with no file keeps them in memory and
//! writes them on its first save.

use std::path::{Path, PathBuf};

use libertum_export::{Tag, TagCategory, TagSuggestion};

use super::App;

/// The file, in the document's history folder.
const WORK_TAGS_FILE: &str = "ao3-tags.json";

/// The tags one work no longer needs suggested.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct WorkTags {
    /// Copied with Copy tags: already on the work.
    pub added: Vec<Tag>,
    /// Removed with the – button: unrelated to the work.
    pub dismissed: Vec<Tag>,
}

/// The people in a pairing, lowercased and sorted, so "A/B" and "B/A" are the same pair.
fn people(name: &str) -> Vec<String> {
    let mut people: Vec<String> = name
        .split(['/', '&'])
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    people.sort();
    people
}

/// Whether two tags are the same for hiding: same category and name ignoring case; for
/// pairings, the same people in any order and either way of writing them.
fn same(a: &Tag, b: &Tag) -> bool {
    if a.category != b.category {
        return false;
    }
    if a.category == TagCategory::Relationship {
        return people(&a.name) == people(&b.name);
    }
    a.name.trim().eq_ignore_ascii_case(b.name.trim())
}

impl WorkTags {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.dismissed.is_empty()
    }

    /// How many tags are hidden.
    pub fn hidden(&self) -> usize {
        self.added.len() + self.dismissed.len()
    }

    /// Whether `tag` is hidden for this work.
    pub fn hides(&self, tag: &Tag) -> bool {
        self.added
            .iter()
            .chain(&self.dismissed)
            .any(|t| same(t, tag))
    }

    /// Records tags copied to the work. The rating and the warnings are chosen on AO3's
    /// form, not copied, so they are not recorded.
    pub fn add(&mut self, tags: impl IntoIterator<Item = Tag>) {
        for tag in tags {
            if tag.category.fixed_names().is_none() && !self.added.iter().any(|t| same(t, &tag)) {
                self.added.push(tag);
            }
        }
    }

    /// Records a tag removed as unrelated.
    pub fn dismiss(&mut self, tag: Tag) {
        if !self.dismissed.iter().any(|t| same(t, &tag)) {
            self.dismissed.push(tag);
        }
    }

    /// Leaves out every hidden tag.
    pub fn filter(&self, suggestions: Vec<TagSuggestion>) -> Vec<TagSuggestion> {
        suggestions
            .into_iter()
            .filter(|s| !self.hides(&s.tag))
            .collect()
    }

    fn path(document: &Path) -> PathBuf {
        libertum_library::history_dir(document).join(WORK_TAGS_FILE)
    }

    fn load(document: &Path) -> Self {
        std::fs::read(Self::path(document))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, document: &Path) -> std::io::Result<()> {
        let path = Self::path(document);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        libertum_library::save_atomic(&path, |temporary| std::fs::write(temporary, &bytes)).map_err(
            |error| match error {
                libertum_library::SaveError::Write(error)
                | libertum_library::SaveError::Permissions(error)
                | libertum_library::SaveError::Replace(error) => error,
            },
        )
    }
}

impl App {
    /// Makes `work_tags` the open document's: loaded from its file when the document changed,
    /// and written for the first time when an unsaved document got a file.
    pub(super) fn sync_work_tags(&mut self) {
        let doc_id = self.doc.as_ref().map(|doc| doc.id());
        let key = (doc_id, self.file.clone());
        if self.work_tags_for == key {
            return;
        }
        let same_document = self.work_tags_for.0 == doc_id;
        match (&self.file, same_document && self.work_tags_for.1.is_none()) {
            // The document was just saved for the first time: keep what it hid.
            (Some(path), true) => {
                if !self.work_tags.is_empty()
                    && let Err(error) = self.work_tags.save(path)
                {
                    tracing::warn!(%error, "could not keep the work's hidden tags");
                }
            }
            (Some(path), false) => self.work_tags = WorkTags::load(path),
            (None, _) => self.work_tags = WorkTags::default(),
        }
        self.work_tags_for = key;
    }

    /// Writes `work_tags` beside the document (when it has a file).
    pub(super) fn save_work_tags(&self) {
        if let Some(path) = &self.file
            && let Err(error) = self.work_tags.save(path)
        {
            tracing::warn!(%error, "could not keep the work's hidden tags");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairings_match_in_any_order_and_fixed_choices_are_not_recorded() {
        let mut work = WorkTags::default();
        work.add([
            Tag::new("Mara Ellison/Tomas Reyes", TagCategory::Relationship),
            Tag::new("Fluff", TagCategory::Freeform),
            Tag::new("Mature", TagCategory::Rating),
        ]);
        assert_eq!(work.added.len(), 2);
        assert!(work.hides(&Tag::new(
            "Tomas Reyes & Mara Ellison",
            TagCategory::Relationship
        )));
        assert!(work.hides(&Tag::new("fluff", TagCategory::Freeform)));
        assert!(
            !work.hides(&Tag::new("Fluff", TagCategory::Fandom)),
            "category matters"
        );
        work.dismiss(Tag::new("Harbour Lights", TagCategory::Freeform));
        assert_eq!(work.hidden(), 3);
    }
}
