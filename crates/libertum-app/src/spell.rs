//! Spelling checks with `zspell` and Hunspell-format dictionaries.
//!
//! The British dictionary is the MIT/BSD `wooorm/dictionaries` build of SCOWL; the American
//! one is the SCOWL `en_US` dictionary from LibreOffice's dictionary collection. Both are
//! embedded and loaded lazily, so a document only pays for the dictionary it uses. Checks
//! run off the UI thread and only cover the paragraphs near the viewport.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use directories::ProjectDirs;
use libertum_model::BlockId;
use serde::{Deserialize, Serialize};
use zspell::Dictionary;

/// The dictionaries Libertum ships. British is the default: the interface and the project's
/// spelling are British/Singapore English.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    British,
    American,
}

impl Language {
    pub fn label(self) -> &'static str {
        match self {
            Self::British => "English (British)",
            Self::American => "English (American)",
        }
    }

    fn files(self) -> (&'static str, &'static str) {
        match self {
            Self::British => (
                include_str!("../assets/dictionaries/en_GB.aff"),
                include_str!("../assets/dictionaries/en_GB.dic"),
            ),
            Self::American => (
                include_str!("../assets/dictionaries/en_US.aff"),
                include_str!("../assets/dictionaries/en_US.dic"),
            ),
        }
    }
}

/// One misspelt word in a paragraph, with byte offsets into its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub block: BlockId,
    pub start: usize,
    pub end: usize,
    pub word: String,
}

/// The global dictionaries, built on first use. Building a dictionary takes a moment, so the
/// UI never does it on a keystroke.
fn dictionary(language: Language) -> Result<Arc<Dictionary>, String> {
    static CACHE: OnceLock<Mutex<HashMap<Language, Arc<Dictionary>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(dict) = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&language)
        .cloned()
    {
        return Ok(dict);
    }
    let (aff, dic) = language.files();
    // The British dictionary file starts with a UTF-8 byte-order mark; zspell's parser
    // expects the count on the first line.
    let aff = aff.strip_prefix('\u{feff}').unwrap_or(aff);
    let dic = dic.strip_prefix('\u{feff}').unwrap_or(dic);
    let dict = zspell::builder()
        .config_str(aff)
        .dict_str(dic)
        .build()
        .map_err(|error| error.to_string())?;
    let dict = Arc::new(dict);
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(language, dict.clone());
    Ok(dict)
}

/// Checks `text` and returns the misspelt spans, skipping the personal and per-document
/// ignore lists (both held lowercased).
pub fn check(
    text: &str,
    language: Language,
    personal: &HashSet<String>,
    ignored: &HashSet<String>,
) -> Result<Vec<(usize, usize, String)>, String> {
    let dict = dictionary(language)?;
    let mut out = Vec::new();
    for (start, word) in dict.check_indices(text) {
        let lower = word.to_lowercase();
        if personal.contains(&lower) || ignored.contains(&lower) {
            continue;
        }
        out.push((start, start + word.len(), word.to_owned()));
    }
    Ok(out)
}

/// Checks the paragraphs in one background pass. Each paragraph is `(block id, text)`.
pub fn check_paragraphs(
    paragraphs: &[(BlockId, String)],
    language: Language,
    personal: &HashSet<String>,
    ignored: &HashSet<String>,
) -> Result<Vec<Span>, String> {
    let mut out = Vec::new();
    for (block, text) in paragraphs {
        for (start, end, word) in check(text, language, personal, ignored)? {
            out.push(Span {
                block: *block,
                start,
                end,
                word,
            });
        }
    }
    Ok(out)
}

/// Up to five suggestions for `word`, best first. Suggestions are only built when the
/// writer opens the menu, not for every misspelling.
pub fn suggestions(word: &str, language: Language) -> Vec<String> {
    let Ok(dict) = dictionary(language) else {
        return Vec::new();
    };
    dict.entry(word)
        .suggest()
        .unwrap_or_default()
        .into_iter()
        .take(5)
        .map(str::to_owned)
        .collect()
}

/// The personal word list, kept beside the recovery snapshots in the data directory.
#[cfg_attr(test, allow(dead_code))]
pub fn personal_path() -> Option<PathBuf> {
    ProjectDirs::from("", "Libertum", "Libertum").map(|dirs| dirs.data_dir().join("dictionary.txt"))
}

/// Loads the personal word list; a missing file is an empty list.
pub fn load_personal(path: &Path) -> HashSet<String> {
    fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_lowercase)
                .collect()
        })
        .unwrap_or_default()
}

/// Adds a word to the personal dictionary. It is stored lowercased, one word per line, so a
/// capitalised form and its lowercase share one entry.
pub fn add_personal(path: &Path, word: &str) -> Result<(), String> {
    let word = word.trim();
    if word.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    writeln!(file, "{}", word.to_lowercase()).map_err(|error| error.to_string())
}

/// The word around a byte offset, for the right-click menu. Apostrophes and hyphens count as
/// part of a word; surrounding punctuation does not.
pub fn word_at(text: &str, offset: usize) -> Option<(usize, usize, String)> {
    let offset = offset.min(text.len());
    let is_word = |c: char| c.is_alphabetic() || c == '\'' || c == '\u{2019}' || c == '-';
    let mut start = offset;
    while start > 0 {
        let Some(c) = text[..start].chars().next_back() else {
            break;
        };
        if !is_word(c) {
            break;
        }
        start -= c.len_utf8();
    }
    let mut end = offset;
    while end < text.len() {
        let Some(c) = text[end..].chars().next() else {
            break;
        };
        if !is_word(c) {
            break;
        }
        end += c.len_utf8();
    }
    let word = &text[start..end];
    let trimmed = word.trim_matches(['\'', '\u{2019}', '-']);
    if trimmed.is_empty() {
        return None;
    }
    let leading = word.len() - word.trim_start_matches(['\'', '\u{2019}', '-']).len();
    let start = start + leading;
    Some((start, start + trimmed.len(), trimmed.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_bundled_dictionaries_load_and_understand_their_spelling() {
        assert!(
            check(
                "The colour is clear.",
                Language::British,
                &HashSet::new(),
                &HashSet::new()
            )
            .expect("British dictionary")
            .is_empty()
        );
        assert!(
            check(
                "The color is clear.",
                Language::American,
                &HashSet::new(),
                &HashSet::new()
            )
            .expect("American dictionary")
            .is_empty()
        );
        let british = check(
            "The color is clear.",
            Language::British,
            &HashSet::new(),
            &HashSet::new(),
        )
        .expect("British dictionary");
        assert!(
            british.iter().any(|(_, _, word)| word == "color"),
            "the British dictionary should question the American spelling: {british:?}"
        );
    }

    #[test]
    fn a_misspelling_has_its_byte_range_and_word() {
        let errors = check(
            "A smple sentence.",
            Language::British,
            &HashSet::new(),
            &HashSet::new(),
        )
        .expect("dictionary");
        assert_eq!(errors, vec![(2, 7, "smple".to_owned())]);
    }

    #[test]
    fn personal_and_document_ignores_are_skipped() {
        let mut personal = HashSet::new();
        personal.insert("smple".to_owned());
        assert!(
            check(
                "A smple sentence.",
                Language::British,
                &personal,
                &HashSet::new()
            )
            .expect("dictionary")
            .is_empty()
        );
        let mut ignored = HashSet::new();
        ignored.insert("smple".to_owned());
        assert!(
            check(
                "A smple sentence.",
                Language::British,
                &HashSet::new(),
                &ignored
            )
            .expect("dictionary")
            .is_empty()
        );
    }

    #[test]
    fn suggestions_offer_the_obvious_correction() {
        let suggestions = suggestions("colur", Language::British);
        assert!(
            suggestions
                .iter()
                .any(|word| word.eq_ignore_ascii_case("colour")),
            "expected colour among {suggestions:?}"
        );
    }

    #[test]
    fn the_personal_list_round_trips() {
        let dir = std::env::temp_dir().join(format!("libertum-spell-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temporary folder");
        let path = dir.join("dictionary.txt");
        add_personal(&path, "Colur").expect("add");
        assert!(load_personal(&path).contains("colur"));
        let _ = fs::remove_dir_all(&dir);
    }
}
