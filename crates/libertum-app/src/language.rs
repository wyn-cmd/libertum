//! The language a document is written in, as the Document pane offers it.
//!
//! English comes in two spellings, and Ukrainian and Russian are Cyrillic; each has a
//! dictionary (see [`crate::spell`]). Chinese, Japanese and Korean use ideographs and have none,
//! so a document in one of them is not spell-checked, but its language still matters: it is
//! stored in the file, and layout picks the regional font from it, because the same Han
//! character is drawn differently in each language (`libertum_layout::script`).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::spell;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocLanguage {
    #[default]
    British,
    American,
    ChineseSimplified,
    ChineseTraditional,
    Japanese,
    Korean,
    Ukrainian,
    Russian,
}

/// A choice for the second language: none, or one of the languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecondLanguage(pub Option<DocLanguage>);

impl SecondLanguage {
    /// Every choice, in the order the dropdown lists them: none first.
    pub const ALL: &'static [Self] = &[
        Self(None),
        Self(Some(DocLanguage::British)),
        Self(Some(DocLanguage::American)),
        Self(Some(DocLanguage::ChineseSimplified)),
        Self(Some(DocLanguage::ChineseTraditional)),
        Self(Some(DocLanguage::Japanese)),
        Self(Some(DocLanguage::Korean)),
        Self(Some(DocLanguage::Ukrainian)),
        Self(Some(DocLanguage::Russian)),
    ];
}

impl fmt::Display for SecondLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(language) => f.write_str(language.label()),
            None => f.write_str("None"),
        }
    }
}

impl DocLanguage {
    /// Every choice, in the order the dropdown lists them.
    pub const ALL: &'static [Self] = &[
        Self::British,
        Self::American,
        Self::ChineseSimplified,
        Self::ChineseTraditional,
        Self::Japanese,
        Self::Korean,
        Self::Ukrainian,
        Self::Russian,
    ];

    /// The name shown in the dropdown: English first, then each language in its own script
    /// so a reader can find it whatever the interface language is.
    pub fn label(self) -> &'static str {
        match self {
            Self::British => "English (British)",
            Self::American => "English (American)",
            Self::ChineseSimplified => "Chinese, Simplified \u{2013} 简体中文",
            Self::ChineseTraditional => "Chinese, Traditional \u{2013} 繁體中文",
            Self::Japanese => "Japanese \u{2013} 日本語",
            Self::Korean => "Korean \u{2013} 한국어",
            Self::Ukrainian => "Ukrainian \u{2013} Українська",
            Self::Russian => "Russian \u{2013} Русский",
        }
    }

    /// The BCP 47 tag a file stores: the ones Word writes for these languages.
    pub fn tag(self) -> &'static str {
        match self {
            Self::British => "en-GB",
            Self::American => "en-US",
            Self::ChineseSimplified => "zh-CN",
            Self::ChineseTraditional => "zh-TW",
            Self::Japanese => "ja-JP",
            Self::Korean => "ko-KR",
            Self::Ukrainian => "uk-UA",
            Self::Russian => "ru-RU",
        }
    }

    /// The choice a file's language tag names, if it names one of these. A bare `en`, or a
    /// language with no entry here, gives `None` and the caller uses the default.
    pub fn from_tag(tag: &str) -> Option<Self> {
        if let Some(english) = spell::Language::from_tag(tag) {
            return Some(Self::from(english));
        }
        match libertum_layout::script::locale_for_tag(tag)? {
            "ja" => Some(Self::Japanese),
            "ko" => Some(Self::Korean),
            "zh-TW" => Some(Self::ChineseTraditional),
            _ => Some(Self::ChineseSimplified),
        }
    }

    /// The spelling dictionary for this language: English, Ukrainian and Russian have one.
    pub fn dictionary(self) -> Option<spell::Language> {
        match self {
            Self::British => Some(spell::Language::British),
            Self::American => Some(spell::Language::American),
            Self::Ukrainian => Some(spell::Language::Ukrainian),
            Self::Russian => Some(spell::Language::Russian),
            _ => None,
        }
    }
}

impl From<spell::Language> for DocLanguage {
    fn from(language: spell::Language) -> Self {
        match language {
            spell::Language::British => Self::British,
            spell::Language::American => Self::American,
            spell::Language::Ukrainian => Self::Ukrainian,
            spell::Language::Russian => Self::Russian,
        }
    }
}

impl fmt::Display for DocLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_round_trips_through_its_tag() {
        for language in DocLanguage::ALL {
            assert_eq!(DocLanguage::from_tag(language.tag()), Some(*language));
        }
    }

    #[test]
    fn other_programs_tags_map_to_the_nearest_choice() {
        for (tag, expected) in [
            ("en-AU", Some(DocLanguage::British)),
            ("en_US", Some(DocLanguage::American)),
            ("zh", Some(DocLanguage::ChineseSimplified)),
            ("zh-Hans", Some(DocLanguage::ChineseSimplified)),
            ("zh-SG", Some(DocLanguage::ChineseSimplified)),
            ("zh-Hant", Some(DocLanguage::ChineseTraditional)),
            ("zh-HK", Some(DocLanguage::ChineseTraditional)),
            ("ja", Some(DocLanguage::Japanese)),
            ("ko", Some(DocLanguage::Korean)),
            ("uk", Some(DocLanguage::Ukrainian)),
            ("uk_UA", Some(DocLanguage::Ukrainian)),
            ("ru-RU", Some(DocLanguage::Russian)),
            ("en", None),
            ("be", None),
            ("fr-FR", None),
            ("de", None),
            ("", None),
            ("ja\"><x", None),
        ] {
            assert_eq!(DocLanguage::from_tag(tag), expected, "{tag:?}");
        }
    }

    #[test]
    fn only_the_east_asian_languages_have_no_dictionary() {
        assert_eq!(
            DocLanguage::American.dictionary(),
            Some(spell::Language::American)
        );
        assert_eq!(
            DocLanguage::Ukrainian.dictionary(),
            Some(spell::Language::Ukrainian)
        );
        assert_eq!(
            DocLanguage::Russian.dictionary(),
            Some(spell::Language::Russian)
        );
        for language in [
            DocLanguage::ChineseSimplified,
            DocLanguage::ChineseTraditional,
            DocLanguage::Japanese,
            DocLanguage::Korean,
        ] {
            assert_eq!(language.dictionary(), None);
        }
    }

    #[test]
    fn the_saved_names_are_stable() {
        // Written to settings.toml for RTF and Markdown files.
        let json = |language: DocLanguage| serde_json::to_string(&language).expect("json");
        assert_eq!(json(DocLanguage::British), "\"british\"");
        assert_eq!(
            json(DocLanguage::ChineseSimplified),
            "\"chinese-simplified\""
        );
    }
}
