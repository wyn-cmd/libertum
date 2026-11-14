//! Persistent user settings, stored as TOML in the platform config directory.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::spell::Language as SpellLanguage;
use crate::state::{Inspector, Units};

pub const SIDEBAR_MIN_WIDTH: f32 = 240.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 480.0;
/// Pages' inspector is 260 points wide.
pub const DEFAULT_SIDEBAR_WIDTH: f32 = 260.0;

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("no platform configuration directory is available")]
    NoConfigDir,
    #[error("could not read or write the settings file: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not parse the settings file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("could not serialise the settings: {0}")]
    Serialise(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefaultFormat {
    #[default]
    Odt,
    Docx,
    Rtf,
    Markdown,
    /// The native `.lbtd` format: lossless, and the only one that keeps pageless and zoom.
    Native,
}

/// The units the inspector shows for lengths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitsPreference {
    /// Centimetres or inches, from the system locale.
    #[default]
    Auto,
    Centimetres,
    Inches,
}

/// A version the writer asked not to be told about. `until` is `None` for "skip this
/// version", or a Unix time after which the prompt may appear again.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateMute {
    pub version: String,
    pub until: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePreference,
    pub default_format: DefaultFormat,
    pub sidebar_collapsed: bool,
    pub sidebar_width: f32,
    /// Which inspector the sidebar shows (Format or Document).
    pub inspector: Inspector,
    /// The floating page and word count pill over the canvas.
    pub show_word_count: bool,
    /// The frame-rate and frame-time overlay in the top-right of the canvas. Off by default:
    /// it keeps the window redrawing while it is shown, so it costs frames itself.
    pub show_performance: bool,
    /// Smooth page text and shapes. Off draws hard pixel edges on the page (the PDF export is
    /// unaffected).
    pub antialiasing: bool,
    pub autosave_secs: u32,
    pub recent_files: Vec<PathBuf>,
    /// The folder the Library window lists (usually inside a synced folder), if one is chosen.
    pub library_folder: Option<PathBuf>,
    /// Measurement units in the inspector (Auto follows the locale).
    pub units: UnitsPreference,
    /// Which heading levels start a chapter in AO3 and EPUB exports.
    pub chapter_headings: libertum_export::ChapterLevels,
    /// The local AO3 tag vocabulary: what suggested tags are ranked against. Nothing
    /// leaves this file; see `libertum_export::tags`.
    pub tags: Vec<libertum_export::Tag>,
    /// Ask GitHub once a day whether a newer release exists. On by default; the check only
    /// reads the public releases API and never installs anything.
    pub check_for_updates: bool,
    /// Underline misspellings while typing. On by default; nothing leaves the machine.
    pub spellcheck: bool,
    /// The dictionary used for the open document's spelling checks.
    pub spellcheck_language: SpellLanguage,
    /// When the last update check was attempted (Unix seconds), so launches do not check
    /// more than once a day.
    pub last_update_check: Option<u64>,
    /// A release version the writer muted, with the time the mute ends (`None` = forever
    /// for that version; a newer version still prompts).
    pub update_mute: Option<UpdateMute>,
}

impl UnitsPreference {
    /// The inspector units this preference means (Auto follows the locale).
    pub fn to_units(self) -> Units {
        match self {
            Self::Auto => Units::detect(),
            Self::Centimetres => Units::Centimetres,
            Self::Inches => Units::Inches,
        }
    }
}

impl Settings {
    /// The measurement units the inspector should use.
    pub fn units(&self) -> Units {
        self.units.to_units()
    }

    /// Whether a prompt for `version` should be shown now. A mute for another version does
    /// not silence a newer one, and turning automatic checks off silences all of them.
    pub fn wants_update_prompt(&self, version: &str, now: u64) -> bool {
        if !self.check_for_updates {
            return false;
        }
        match &self.update_mute {
            Some(mute) if mute.version == version => mute.until.is_some_and(|until| now >= until),
            _ => true,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            default_format: DefaultFormat::Odt,
            sidebar_collapsed: false,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            inspector: Inspector::Format,
            show_word_count: true,
            show_performance: false,
            antialiasing: true,
            autosave_secs: 60,
            recent_files: Vec::new(),
            library_folder: None,
            units: UnitsPreference::Auto,
            chapter_headings: libertum_export::ChapterLevels::default(),
            tags: Vec::new(),
            check_for_updates: true,
            spellcheck: true,
            spellcheck_language: SpellLanguage::British,
            last_update_check: None,
            update_mute: None,
        }
    }
}

impl Settings {
    pub fn to_toml(&self) -> Result<String, SettingsError> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn from_toml(text: &str) -> Result<Self, SettingsError> {
        Ok(toml::from_str(text)?)
    }

    /// Sidebar width clamped to a usable range (a hand-edited file may hold anything).
    pub fn clamped_sidebar_width(&self) -> f32 {
        if self.sidebar_width.is_finite() {
            self.sidebar_width
                .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
        } else {
            DEFAULT_SIDEBAR_WIDTH
        }
    }

    pub fn default_path() -> Result<PathBuf, SettingsError> {
        let dirs =
            ProjectDirs::from("", "Libertum", "Libertum").ok_or(SettingsError::NoConfigDir)?;
        Ok(dirs.config_dir().join("settings.toml"))
    }

    pub fn load_from(path: &Path) -> Result<Self, SettingsError> {
        Self::from_toml(&std::fs::read_to_string(path)?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.to_toml()?)?;
        Ok(())
    }

    /// Load from the platform config directory. A missing file is normal on first run; any
    /// other failure is logged. Either way the app starts with usable defaults.
    pub fn load_or_default() -> (Self, Option<PathBuf>) {
        let path = match Self::default_path() {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(%error, "settings location unavailable; using defaults");
                return (Self::default(), None);
            }
        };
        let settings = match Self::load_from(&path) {
            Ok(settings) => settings,
            Err(SettingsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(path = %path.display(), "no settings file yet; using defaults");
                Self::default()
            }
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "could not load settings; using defaults");
                Self::default()
            }
        };
        (settings, Some(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_plan() {
        let s = Settings::default();
        assert_eq!(s.default_format, DefaultFormat::Odt);
        assert!(!s.show_performance, "the overlay is off until asked for");
        assert!(s.antialiasing, "page text is smooth until switched off");
        assert_eq!(s.theme, ThemePreference::System);
        assert!(
            s.check_for_updates,
            "updates are checked until switched off"
        );
    }

    #[test]
    fn round_trips_through_toml() {
        let original = Settings {
            theme: ThemePreference::Dark,
            default_format: DefaultFormat::Docx,
            sidebar_collapsed: true,
            sidebar_width: 320.0,
            inspector: Inspector::Document,
            show_word_count: false,
            show_performance: true,
            antialiasing: false,
            autosave_secs: 30,
            recent_files: vec![PathBuf::from("a.odt"), PathBuf::from("b.docx")],
            library_folder: Some(PathBuf::from("Books")),
            units: UnitsPreference::Inches,
            chapter_headings: libertum_export::ChapterLevels {
                h1: false,
                h2: true,
                h3: true,
                h4: false,
            },
            tags: vec![
                libertum_export::Tag::new(
                    "Teen And Up Audiences",
                    libertum_export::TagCategory::Rating,
                ),
                libertum_export::Tag::new(
                    "Enemies to Lovers",
                    libertum_export::TagCategory::Relationship,
                ),
            ],
            check_for_updates: false,
            spellcheck: false,
            spellcheck_language: crate::spell::Language::American,
            last_update_check: Some(1_700_000_000),
            update_mute: Some(UpdateMute {
                version: "0.1.3".to_owned(),
                until: Some(1_700_086_400),
            }),
        };
        let text = original.to_toml().expect("serialise");
        assert_eq!(Settings::from_toml(&text).expect("parse"), original);
    }

    #[test]
    fn update_prompts_respect_the_mute() {
        let mut s = Settings::default();
        assert!(s.wants_update_prompt("0.1.3", 100));

        s.update_mute = Some(UpdateMute {
            version: "0.1.3".to_owned(),
            until: Some(200),
        });
        assert!(!s.wants_update_prompt("0.1.3", 100), "muted until 200");
        assert!(s.wants_update_prompt("0.1.3", 200), "the mute has ended");
        assert!(s.wants_update_prompt("0.1.4", 100), "a newer release");

        s.update_mute = Some(UpdateMute {
            version: "0.1.3".to_owned(),
            until: None,
        });
        assert!(!s.wants_update_prompt("0.1.3", 10_000));
        assert!(s.wants_update_prompt("0.2.0", 10_000));

        s.check_for_updates = false;
        assert!(!s.wants_update_prompt("0.2.0", 10_000));
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s = Settings::from_toml("sidebar_collapsed = true\n").expect("parse");
        assert!(s.sidebar_collapsed);
        assert_eq!(s.autosave_secs, 60);
        assert_eq!(
            s.chapter_headings,
            libertum_export::ChapterLevels::default()
        );
    }

    #[test]
    fn new_fields_default_for_old_files() {
        let s = Settings::from_toml(
            "theme = \"dark\"
",
        )
        .expect("parse");
        assert_eq!(s.inspector, Inspector::Format);
        assert!(s.show_word_count);
        assert!(s.tags.is_empty(), "an old file has no vocabulary yet");
    }

    #[test]
    fn the_tag_vocabulary_is_stored_as_a_readable_list() {
        let s = Settings {
            tags: vec![
                libertum_export::Tag::new("Mature", libertum_export::TagCategory::Rating),
                libertum_export::Tag::new("Slow Burn", libertum_export::TagCategory::Freeform),
            ],
            ..Settings::default()
        };
        let text = s.to_toml().expect("serialise");
        assert!(text.contains("[[tags]]"), "{text}");
        assert!(text.contains("category = \"rating\""), "{text}");
        assert!(text.contains("category = \"freeform\""), "{text}");
        assert_eq!(Settings::from_toml(&text).expect("parse").tags, s.tags);
    }

    #[test]
    fn inspector_choice_is_persisted_as_text() {
        let s = Settings {
            inspector: Inspector::Document,
            show_word_count: false,
            ..Settings::default()
        };
        let text = s.to_toml().expect("serialise");
        assert!(text.contains("inspector = \"document\""));
        assert!(text.contains("show_word_count = false"));
    }

    #[test]
    fn invalid_toml_is_an_error() {
        assert!(Settings::from_toml("theme = 7").is_err());
    }

    #[test]
    fn file_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("libertum-settings-test-{}", std::process::id()));
        let path = dir.join("nested").join("settings.toml");
        let original = Settings {
            sidebar_collapsed: true,
            ..Settings::default()
        };
        original.save_to(&path).expect("save");
        assert_eq!(Settings::load_from(&path).expect("load"), original);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
