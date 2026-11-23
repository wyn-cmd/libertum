//! Spelling checks with `spellbook` and Hunspell-format dictionaries.
//!
//! The British dictionary is the MIT/BSD `wooorm/dictionaries` build of SCOWL; the American
//! one is the SCOWL `en_US` dictionary from LibreOffice's dictionary collection; the Ukrainian
//! (MPL 1.1) and Russian (BSD-style) ones come from the same collection. All are embedded and
//! loaded lazily, so a document only pays for the dictionary it uses. Checks run off the UI
//! thread and only cover the paragraphs near the viewport.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use directories::ProjectDirs;
use libertum_model::BlockId;
use serde::{Deserialize, Serialize};
use spellbook::Dictionary;

/// The dictionaries Libertum ships. British is the default: the interface and the project's
/// spelling are British/Singapore English.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    British,
    American,
    Ukrainian,
    Russian,
}

impl Language {
    /// The BCP 47 tag a document stores for this dictionary.
    pub fn tag(self) -> &'static str {
        match self {
            Self::British => "en-GB",
            Self::American => "en-US",
            Self::Ukrainian => "uk-UA",
            Self::Russian => "ru-RU",
        }
    }

    /// The dictionary for a document's language tag, if one fits. `en-US` is American; any
    /// other regional English (`en-GB`, `en-AU`, `en-NZ`, `en-ZA`, `en-IN`, `en-SG`,
    /// `en-CA` and so on) spells closest to British. A bare `en` does not say which, so it
    /// gives `None` (the caller's default), as does a language with no dictionary here.
    /// Ukrainian and Russian have one whatever the region.
    pub fn from_tag(tag: &str) -> Option<Self> {
        let tag = libertum_model::language_tag(tag)?;
        let mut parts = tag.split('-');
        match parts.next()? {
            "uk" => return Some(Self::Ukrainian),
            "ru" => return Some(Self::Russian),
            "en" => {}
            _ => return None,
        }
        match parts.find(|part| part.len() == 2 || part.chars().all(|c| c.is_ascii_digit())) {
            Some("US") => Some(Self::American),
            Some(_) => Some(Self::British),
            None => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::British => "English (British)",
            Self::American => "English (American)",
            Self::Ukrainian => "Ukrainian",
            Self::Russian => "Russian",
        }
    }

    /// Whether `letter` belongs to the script this dictionary is for. Words in another script
    /// are not checked: a Ukrainian dictionary knows no English word, and an English one would
    /// underline every Cyrillic, Greek, Korean or Chinese word.
    fn is_script_letter(self, letter: char) -> bool {
        match self {
            Self::British | Self::American => libertum_layout::script::is_latin_letter(letter),
            // Cyrillic and Cyrillic Supplement, plus the combining acute that marks stress.
            Self::Ukrainian | Self::Russian => {
                matches!(u32::from(letter), 0x0400..=0x052F | 0x1C80..=0x1C8F | 0x0301)
            }
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
            Self::Ukrainian => (
                include_str!("../assets/dictionaries/uk_UA.aff"),
                include_str!("../assets/dictionaries/uk_UA.dic"),
            ),
            Self::Russian => (
                include_str!("../assets/dictionaries/ru_RU.aff"),
                include_str!("../assets/dictionaries/ru_RU.dic"),
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

/// The global dictionary, built on first use. Building one takes from 10 ms (English) to about
/// 110 ms (Ukrainian) and holds from 3 MB to about 40 MB, so the UI never does it on a keystroke, and it is built exactly once: the first
/// background spell check to ask builds it while the others wait for the lock. (Checking the
/// cache and building outside the lock let every pool thread that asked while it was empty
/// build its own copy, up to sixteen at once.) Only the language in use is kept.
fn dictionary(language: Language) -> Result<Arc<Dictionary>, String> {
    static CACHE: OnceLock<Mutex<HashMap<Language, Arc<Dictionary>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    dictionary_in(cache, language, build_dictionary)
}

/// [`dictionary`] over an explicit cache and builder, so a test can count the builds.
fn dictionary_in(
    cache: &Mutex<HashMap<Language, Arc<Dictionary>>>,
    language: Language,
    build: impl FnOnce(Language) -> Result<Dictionary, String>,
) -> Result<Arc<Dictionary>, String> {
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(dict) = cache.get(&language) {
        return Ok(dict.clone());
    }
    let dict = Arc::new(build(language)?);
    // A checker already running keeps its own `Arc` to the old language's dictionary.
    cache.clear();
    cache.insert(language, dict.clone());
    Ok(dict)
}

fn build_dictionary(language: Language) -> Result<Dictionary, String> {
    let (aff, dic) = language.files();
    // The British dictionary file starts with a UTF-8 byte-order mark; the parser expects the
    // count on the first line.
    let aff = aff.strip_prefix('\u{feff}').unwrap_or(aff);
    let dic = dic.strip_prefix('\u{feff}').unwrap_or(dic);
    Dictionary::new(aff, dic).map_err(|error| error.to_string())
}

/// Builds the dictionary for `language` on a background thread, so the first check after a
/// document opens does not wait for it (building takes from about 0.1 s to a few seconds
/// while a large document is being laid out).
pub fn warm(language: Language) {
    let _ = std::thread::Builder::new()
        .name("libertum-spell-warm".into())
        .spawn(move || {
            if let Err(error) = dictionary(language) {
                tracing::warn!(%error, "could not load the dictionary");
            }
        });
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
    Ok(check_with(&dict, language, text, personal, ignored))
}

/// The check itself, with the dictionary already in hand. The personal and ignore lists are
/// usually empty, so a word is only lowercased when there is a list to compare it with.
fn check_with(
    dict: &Dictionary,
    language: Language,
    text: &str,
    personal: &HashSet<String>,
    ignored: &HashSet<String>,
) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for (start, word) in words(text) {
        // A lone letter or a word with a digit in it (`3rd`, `B2B`, a version number) is not
        // a word to correct, and a dictionary reports some of them as misspellings.
        let mut chars = word.chars();
        let single = chars.next().is_some() && chars.next().is_none();
        if single || word.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        // Only words in the dictionary's own script are checked: an English dictionary knows
        // nothing of Korean, Japanese, Chinese, Greek or Cyrillic words, and a Ukrainian one
        // nothing of English, so each would underline every foreign word.
        let mut letters = word.chars().filter(|c| c.is_alphabetic()).peekable();
        if letters.peek().is_none() || !letters.all(|c| language.is_script_letter(c)) {
            continue;
        }
        if is_spelt(dict, word) {
            continue;
        }
        if !personal.is_empty() || !ignored.is_empty() {
            let lower = word.to_lowercase();
            if personal.contains(&lower) || ignored.contains(&lower) {
                continue;
            }
        }
        out.push((start, start + word.len(), word.to_owned()));
    }
    out
}

/// Whether the dictionary accepts `word`. Typographic apostrophes are read as the plain one
/// the dictionaries are written with, and a hyphenated word the dictionary does not know as
/// a whole is accepted when each of its parts is a word.
fn is_spelt(dict: &Dictionary, word: &str) -> bool {
    let plain: String = word
        .chars()
        .map(|c| {
            if matches!(c, '\u{2019}' | '\u{02BC}') {
                '\''
            } else {
                c
            }
        })
        .collect();
    if dict.check(&plain) {
        return true;
    }
    plain.contains('-')
        && plain
            .split('-')
            .filter(|part| !part.is_empty())
            .all(|part| part.chars().count() < 2 || dict.check(part))
}

/// The words of `text` with their byte offsets. A word is a run of letters, digits and
/// combining marks; an apostrophe or hyphen between two of them stays inside it (`don't`,
/// `well-known`, `п'ять`), and one at either end does not.
fn words(text: &str) -> Vec<(usize, &str)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let is_word = |c: char| c.is_alphanumeric() || matches!(u32::from(c), 0x0300..=0x036F);
    let is_joiner = |c: char| matches!(c, '\'' | '\u{2019}' | '\u{02BC}' | '-' | '\u{2011}');
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !is_word(chars[i].1) {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < chars.len() {
            let (_, c) = chars[i];
            if is_word(c)
                || (is_joiner(c) && chars.get(i + 1).is_some_and(|&(_, next)| is_word(next)))
            {
                i += 1;
            } else {
                break;
            }
        }
        let from = chars[start].0;
        let to = chars.get(i).map_or(text.len(), |&(at, _)| at);
        out.push((from, &text[from..to]));
    }
    out
}

/// Checks the paragraphs in one background pass. Each paragraph is `(block id, text)`. The
/// dictionary is looked up once, not once per paragraph.
pub fn check_paragraphs(
    paragraphs: &[(BlockId, String)],
    language: Language,
    personal: &HashSet<String>,
    ignored: &HashSet<String>,
) -> Result<Vec<Span>, String> {
    let dict = dictionary(language)?;
    let mut out = Vec::new();
    for (block, text) in paragraphs {
        for (start, end, word) in check_with(&dict, language, text, personal, ignored) {
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

/// Checks `text` against several dictionaries (a document in two languages). Each dictionary
/// only judges words in its own script, so a word is misspelt when every dictionary that
/// judges it says so: a Ukrainian word is the Ukrainian dictionary's to judge, and a Latin
/// word is judged by both English dictionaries, which must both reject it.
pub fn check_any(
    text: &str,
    languages: &[Language],
    personal: &HashSet<String>,
    ignored: &HashSet<String>,
) -> Result<Vec<(usize, usize, String)>, String> {
    let mut per_language = Vec::with_capacity(languages.len());
    for language in languages {
        per_language.push((*language, check(text, *language, personal, ignored)?));
    }
    let mut out: Vec<(usize, usize, String)> = Vec::new();
    for (language, errors) in &per_language {
        for error in errors {
            let rejected_by_all = per_language
                .iter()
                .filter(|(other, _)| other != language)
                .all(|(other, other_errors)| {
                    // A dictionary that does not judge this word cannot rescue it.
                    let judges = error
                        .2
                        .chars()
                        .filter(|c| c.is_alphabetic())
                        .all(|c| other.is_script_letter(c));
                    !judges || other_errors.contains(error)
                });
            if rejected_by_all && !out.contains(error) {
                out.push(error.clone());
            }
        }
    }
    out.sort_by_key(|(start, _, _)| *start);
    Ok(out)
}

/// [`check_paragraphs`] for several dictionaries (see [`check_any`]).
pub fn check_paragraphs_any(
    paragraphs: &[(BlockId, String)],
    languages: &[Language],
    personal: &HashSet<String>,
    ignored: &HashSet<String>,
) -> Result<Vec<Span>, String> {
    if let [only] = languages {
        return check_paragraphs(paragraphs, *only, personal, ignored);
    }
    let mut out = Vec::new();
    for (block, text) in paragraphs {
        for (start, end, word) in check_any(text, languages, personal, ignored)? {
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

/// Suggestions for `word` from several dictionaries, taken in turn so each language's best
/// come first; up to five.
pub fn suggestions_any(word: &str, languages: &[Language]) -> Vec<String> {
    // Only the dictionaries that judge the word's script have anything to suggest.
    let lists: Vec<Vec<String>> = languages
        .iter()
        .filter(|l| {
            word.chars()
                .filter(|c| c.is_alphabetic())
                .all(|c| l.is_script_letter(c))
        })
        .map(|l| suggestions(word, *l))
        .collect();
    let mut out: Vec<String> = Vec::new();
    for rank in 0..5 {
        for list in &lists {
            if let Some(s) = list.get(rank)
                && !out.contains(s)
            {
                out.push(s.clone());
            }
        }
    }
    out.truncate(5);
    out
}

/// Up to five suggestions for `word`, best first. Suggestions are only built when the
/// writer opens the menu, not for every misspelling.
pub fn suggestions(word: &str, language: Language) -> Vec<String> {
    let Ok(dict) = dictionary(language) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    dict.suggest(word, &mut found);
    found.truncate(5);
    found
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
mod tag_tests {
    use std::collections::HashSet;

    use super::{Language, check_any, suggestions_any};

    #[test]
    fn a_word_is_right_if_either_of_two_dictionaries_knows_it() {
        let none = HashSet::new();
        let text = "Hello привіт wrongg привітт";
        let words = |languages: &[Language]| -> Vec<String> {
            check_any(text, languages, &none, &none)
                .expect("check")
                .into_iter()
                .map(|(_, _, w)| w)
                .collect()
        };
        // Each dictionary only judges words in its own script.
        assert_eq!(words(&[Language::British]), ["wrongg"]);
        assert_eq!(words(&[Language::Ukrainian]), ["привітт"]);
        // Together they judge both scripts, and neither rescues the other's misspellings.
        assert_eq!(
            words(&[Language::British, Language::Ukrainian]),
            ["wrongg", "привітт"]
        );
        // Two Englishes: a word is wrong only if both reject it, so a spelling either accepts
        // passes.
        let colour = check_any(
            "colour color wrongg",
            &[Language::British, Language::American],
            &none,
            &none,
        )
        .expect("check");
        assert_eq!(
            colour.iter().map(|e| e.2.as_str()).collect::<Vec<_>>(),
            ["wrongg"]
        );
        assert!(
            check_any(text, &[], &none, &none)
                .expect("check")
                .is_empty()
        );
        // Suggestions come from each language in turn.
        let mixed = suggestions_any("helo", &[Language::British, Language::Ukrainian]);
        assert!(
            mixed.len() <= 5 && mixed.contains(&"hello".to_owned()),
            "{mixed:?}"
        );
    }

    #[test]
    fn tags_map_to_dictionaries_and_back() {
        assert_eq!(Language::British.tag(), "en-GB");
        assert_eq!(Language::American.tag(), "en-US");
        for language in [
            Language::British,
            Language::American,
            Language::Ukrainian,
            Language::Russian,
        ] {
            assert_eq!(Language::from_tag(language.tag()), Some(language));
        }
        assert_eq!(Language::from_tag("en_us"), Some(Language::American));
        for british in [
            "en-AU",
            "en-NZ",
            "en-CA",
            "en-ZA",
            "en-IN",
            "en-SG",
            "en-Latn-GB",
        ] {
            assert_eq!(
                Language::from_tag(british),
                Some(Language::British),
                "{british}"
            );
        }
        // A bare "en" does not say which, and there is no French dictionary.
        assert_eq!(Language::from_tag("uk"), Some(Language::Ukrainian));
        assert_eq!(Language::from_tag("uk_UA"), Some(Language::Ukrainian));
        assert_eq!(Language::from_tag("ru-RU"), Some(Language::Russian));
        for none in ["en", "fr-FR", "de", "", "not a tag", "en-GB\"><x"] {
            assert_eq!(Language::from_tag(none), None, "{none:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_in_other_scripts_are_not_underlined_but_misspelt_english_still_is() {
        let text = "한국어 단어 カタカナ ニャーニャー 你好世界 Привет Ωμέγα teh colour";
        let errors =
            check(text, Language::British, &HashSet::new(), &HashSet::new()).expect("dictionary");
        let words: Vec<&str> = errors.iter().map(|(_, _, w)| w.as_str()).collect();
        assert_eq!(words, ["teh"], "only the English misspelling: {words:?}");
        // The spans still index the text correctly.
        for (start, end, word) in &errors {
            assert_eq!(&text[*start..*end], word);
        }
        // English around East Asian text is still checked.
        let errors = check(
            "This is 日本語 and wrong spelllng.",
            Language::British,
            &HashSet::new(),
            &HashSet::new(),
        )
        .expect("dictionary");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].2, "spelllng");
    }

    fn flagged(text: &str, language: Language) -> Vec<String> {
        check(text, language, &HashSet::new(), &HashSet::new())
            .expect("dictionary")
            .into_iter()
            .map(|(_, _, word)| word)
            .collect()
    }

    #[test]
    fn ukrainian_is_checked_with_its_own_dictionary() {
        // Correct text, with the apostrophe words and the letters English has no key for.
        for text in [
            "Привіт, світе! Це українська мова.",
            "Я маю п'ять м'ячів і одну книжку.",
            "Будь ласка, їжак і єдиний ґанок.",
            // The typographic apostrophe is the same word.
            "п\u{2019}ять м\u{2019}яч",
        ] {
            assert_eq!(
                flagged(text, Language::Ukrainian),
                Vec::<String>::new(),
                "{text}"
            );
        }
        // A real mistake is found, with the right byte range.
        let text = "Це прівіт і мова.";
        let errors =
            check(text, Language::Ukrainian, &HashSet::new(), &HashSet::new()).expect("dictionary");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(&text[errors[0].0..errors[0].1], "прівіт");
        // And it is corrected.
        let suggestions = suggestions("українсва", Language::Ukrainian);
        assert!(
            suggestions.iter().any(|s| s == "українська"),
            "{suggestions:?}"
        );
    }

    #[test]
    fn russian_is_checked_with_its_own_dictionary() {
        assert_eq!(
            flagged("Привет, мир! Это русская книга.", Language::Russian),
            Vec::<String>::new()
        );
        assert_eq!(
            flagged("Это превет и книга.", Language::Russian),
            ["превет"]
        );
        assert!(
            suggestions("превет", Language::Russian)
                .iter()
                .any(|s| s == "привет")
        );
    }

    #[test]
    fn a_word_in_another_script_is_not_checked_in_a_cyrillic_or_latin_document() {
        let mixed = "Це hello 世界 привіт 안녕 і Αθήνα.";
        assert_eq!(flagged(mixed, Language::Ukrainian), Vec::<String>::new());
        // The English dictionary leaves the Cyrillic alone and still finds English mistakes.
        assert_eq!(
            flagged("Це teh слово", Language::British),
            ["teh"],
            "only the English misspelling"
        );
        // And the Ukrainian dictionary does not flag English words in a Ukrainian text.
        assert_eq!(
            flagged("Це слово teh", Language::Ukrainian),
            Vec::<String>::new()
        );
    }

    #[test]
    fn words_keep_inner_apostrophes_and_hyphens_and_drop_outer_ones() {
        let text = "don't 'quoted' well-known -dash- п'ять 3rd а";
        let found: Vec<&str> = words(text).into_iter().map(|(_, w)| w).collect();
        assert_eq!(
            found,
            ["don't", "quoted", "well-known", "dash", "п'ять", "3rd", "а"]
        );
        for (at, word) in words(text) {
            assert_eq!(&text[at..at + word.len()], word);
        }
        assert!(words("").is_empty());
        assert!(words(" ,.! — ").is_empty());
    }

    #[test]
    fn hyphenated_words_are_checked_whole_or_part_by_part() {
        // "well-known" and a compound of two known words are accepted.
        assert_eq!(
            flagged(
                "A well-known fact, and a long-term plan.",
                Language::British
            ),
            Vec::<String>::new()
        );
        // A misspelt part is still caught.
        assert_eq!(
            flagged("A wel-known fact.", Language::British),
            ["wel-known"]
        );
    }

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
    fn threads_asking_together_build_the_dictionary_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let cache = Mutex::new(HashMap::new());
        let builds = AtomicUsize::new(0);
        let dicts: Vec<Arc<Dictionary>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        dictionary_in(&cache, Language::British, |language| {
                            builds.fetch_add(1, Ordering::SeqCst);
                            build_dictionary(language)
                        })
                        .expect("the bundled dictionary builds")
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("no panic"))
                .collect()
        });
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        assert!(dicts.iter().all(|d| Arc::ptr_eq(d, &dicts[0])));
    }

    #[test]
    fn only_the_language_in_use_is_kept() {
        let cache = Mutex::new(HashMap::new());
        dictionary_in(&cache, Language::British, build_dictionary).expect("builds");
        dictionary_in(&cache, Language::American, build_dictionary).expect("builds");
        let cache = cache.lock().expect("not poisoned");
        assert_eq!(cache.len(), 1);
        assert!(cache.contains_key(&Language::American));
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
    fn single_characters_and_punctuation_are_not_misspellings() {
        for text in ["-", "\u{2014}", "'", "\u{2019}", "1", "x"] {
            assert!(
                check(text, Language::British, &HashSet::new(), &HashSet::new())
                    .expect("dictionary")
                    .is_empty(),
                "{text:?} must not be underlined"
            );
        }
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
