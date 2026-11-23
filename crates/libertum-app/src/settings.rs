//! Persistent user settings, stored as TOML in the platform config directory.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::language::DocLanguage;
use crate::spell::Language as SpellLanguage;
use crate::state::{Inspector, Units};

/// The inspector cannot be resized, so the minimum is the default: a width saved by an
/// earlier version (260) is raised to it. 260 clipped the Section pane's Position buttons.
pub const SIDEBAR_MIN_WIDTH: f32 = 320.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 480.0;
/// A little wider than Pages' 260 points, so the longest labels fit their buttons.
pub const DEFAULT_SIDEBAR_WIDTH: f32 = 320.0;

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

/// How many files' languages are remembered by path.
pub const MAX_DOCUMENT_LANGUAGES: usize = 200;

/// The language chosen for one RTF or Markdown file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentLanguage {
    pub path: PathBuf,
    pub language: DocLanguage,
    /// The file's second language, if it has one.
    #[serde(default)]
    pub second: Option<DocLanguage>,
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
    /// Also suggest characters, pairings and keywords found in the chapter's own text, with no
    /// vocabulary needed (`libertum_export::auto_suggestions`). On by default; local only.
    pub auto_suggest_tags: bool,
    /// Also suggest from the bundled starter list of common AO3 additional tags
    /// (`libertum_export::starter_tags`). On by default; local only.
    pub use_starter_tags: bool,
    /// Ask GitHub once a day whether a newer release exists. On by default; the check only
    /// reads the public releases API and never installs anything.
    pub check_for_updates: bool,
    /// Underline misspellings while typing. On by default; nothing leaves the machine.
    pub spellcheck: bool,
    /// The dictionary used for the open document's spelling checks.
    pub spellcheck_language: SpellLanguage,
    /// The language of RTF and Markdown files, by path. Those formats cannot hold a language
    /// of their own, so the app remembers it here; the other formats carry it inside the
    /// file. Newest first, at most [`MAX_DOCUMENT_LANGUAGES`].
    pub document_languages: Vec<DocumentLanguage>,
    /// When the last update check was attempted (Unix seconds), so launches do not check
    /// more than once a day.
    pub last_update_check: Option<u64>,
    /// A release version the writer muted, with the time the mute ends (`None` = forever
    /// for that version; a newer version still prompts).
    pub update_mute: Option<UpdateMute>,
    /// Developer features. Off by default; everything under it (MCP, macros) stays hidden and
    /// inert until this is on. See `docs/mcp-plan.md`.
    pub dev_mode: bool,
    /// MCP connectors (Libertum as an MCP server and client). Can only be on while
    /// `dev_mode` is on: use [`Settings::set_dev_mode`] and [`Settings::set_mcp_enabled`].
    pub mcp_enabled: bool,
    /// Let MCP clients change documents (otherwise they can only read). Off by default, and it
    /// can only be on while MCP is: use [`Settings::set_mcp_allow_edits`]. Even then each
    /// client asks the writer before its first edit.
    pub mcp_allow_edits: bool,
    /// Clients the writer chose "Always allow" for, by the name they announce. The name is the
    /// client's own claim; the bearer token is what keeps strangers out.
    pub mcp_trusted_clients: Vec<String>,
    /// Programs the writer saved to run as MCP servers from the tool runner.
    pub mcp_servers: Vec<McpServerConfig>,
    /// Most memory one recipe script may hold, in megabytes (developer mode). 512 MB by default.
    #[serde(default = "default_script_memory_mb")]
    pub script_memory_mb: u32,
    /// Memory that scripts stopped for using too much may leave behind, all together, before
    /// no script runs until Libertum restarts (megabytes). 1 GB by default.
    #[serde(default = "default_script_total_memory_mb")]
    pub script_total_memory_mb: u32,
}

fn default_script_memory_mb() -> u32 {
    crate::script::DEFAULT_MEMORY_MB
}

fn default_script_total_memory_mb() -> u32 {
    crate::script::DEFAULT_TOTAL_MEMORY_MB
}

/// Most MCP servers the writer can save, so a hand-edited file cannot grow without bound.
pub const MAX_MCP_SERVERS: usize = 20;
/// Longest saved server name or command line, in characters.
pub const MAX_MCP_NAME_CHARS: usize = 60;
pub const MAX_MCP_COMMAND_CHARS: usize = 500;

/// A program Libertum can start as an MCP server and call tools on (developer mode only).
/// It is never started until the writer has approved this exact command line.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    /// What is run, as typed: the program, then its arguments. Quote an argument that holds
    /// spaces with double quotes.
    pub command_line: String,
    /// The command line the writer approved. Changing `command_line` makes this stale, so the
    /// writer is asked again.
    #[serde(default)]
    pub approved_line: Option<String>,
    /// An optional bearer token for a web address, sent as `Authorization: Bearer <token>`. It
    /// is kept in the settings file as plain text (anyone who can read your files can read
    /// it), is only sent to the approved address over https or to this computer, and is never
    /// shown again or written to a log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl std::fmt::Debug for McpServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerConfig")
            .field("name", &self.name)
            .field("command_line", &self.command_line)
            .field("approved_line", &self.approved_line)
            .field("token", &self.token.as_ref().map(|_| "<hidden>"))
            .finish()
    }
}

/// Longest token accepted, in characters.
pub const MAX_MCP_TOKEN_CHARS: usize = 2000;

impl McpServerConfig {
    /// Whether the command line as it stands now is the one the writer approved.
    pub fn is_approved(&self) -> bool {
        self.approved_line.as_deref() == Some(self.command_line.as_str())
    }
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

    /// Turn developer mode on or off. Turning it off also turns off everything under it.
    pub fn set_dev_mode(&mut self, on: bool) {
        self.dev_mode = on;
        if !on {
            self.mcp_enabled = false;
            self.mcp_allow_edits = false;
        }
    }

    /// Turn MCP connectors on or off. Refused (returns `false`) while developer mode is off.
    /// Turning MCP off also turns editing through it off.
    pub fn set_mcp_enabled(&mut self, on: bool) -> bool {
        if on && !self.dev_mode {
            return false;
        }
        self.mcp_enabled = on;
        if !on {
            self.mcp_allow_edits = false;
        }
        true
    }

    /// Let MCP clients edit, or stop them. Refused (returns `false`) unless MCP is on.
    pub fn set_mcp_allow_edits(&mut self, on: bool) -> bool {
        if on && !self.mcp_active() {
            return false;
        }
        self.mcp_allow_edits = on;
        true
    }

    /// Whether MCP clients may be offered edits at all: MCP on and Allow edits on.
    pub fn mcp_edits_allowed(&self) -> bool {
        self.mcp_active() && self.mcp_allow_edits
    }

    /// Whether the writer chose "Always allow" for this client.
    pub fn mcp_trusts(&self, client: &str) -> bool {
        self.mcp_trusted_clients.iter().any(|c| c == client)
    }

    /// Remember "Always allow" for a client (once).
    pub fn trust_mcp_client(&mut self, client: &str) {
        if !self.mcp_trusts(client) {
            self.mcp_trusted_clients.push(client.to_owned());
        }
    }

    /// Forget every "Always allow" (each client asks again).
    pub fn forget_mcp_clients(&mut self) {
        self.mcp_trusted_clients.clear();
    }

    /// Save a server. The name and command line are trimmed; `None` when either is empty, too
    /// long, or the list is full. Returns the new entry's index. It starts unapproved.
    pub fn add_mcp_server(&mut self, name: &str, command_line: &str) -> Option<usize> {
        let (name, command_line) = (name.trim(), command_line.trim());
        if name.is_empty()
            || command_line.is_empty()
            || name.chars().count() > MAX_MCP_NAME_CHARS
            || command_line.chars().count() > MAX_MCP_COMMAND_CHARS
            || self.mcp_servers.len() >= MAX_MCP_SERVERS
        {
            return None;
        }
        self.mcp_servers.push(McpServerConfig {
            name: name.to_owned(),
            command_line: command_line.to_owned(),
            approved_line: None,
            token: None,
        });
        Some(self.mcp_servers.len() - 1)
    }

    /// Keep `token` for a saved server. Only a web address takes one, and it must be one word
    /// of printable characters; an empty token clears it. Returns whether it was kept.
    pub fn set_mcp_server_token(&mut self, index: usize, token: &str) -> bool {
        let token = token.trim();
        let Some(server) = self.mcp_servers.get_mut(index) else {
            return false;
        };
        if token.is_empty() {
            server.token = None;
            return true;
        }
        let lower = server.command_line.trim_start().to_ascii_lowercase();
        let web = lower.starts_with("http://") || lower.starts_with("https://");
        if !web
            || token.chars().count() > MAX_MCP_TOKEN_CHARS
            || token.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return false;
        }
        server.token = Some(token.to_owned());
        true
    }

    pub fn remove_mcp_server(&mut self, index: usize) {
        if index < self.mcp_servers.len() {
            self.mcp_servers.remove(index);
        }
    }

    /// Record that the writer approved the server's command line as it stands now.
    pub fn approve_mcp_server(&mut self, index: usize) {
        if let Some(server) = self.mcp_servers.get_mut(index) {
            server.approved_line = Some(server.command_line.clone());
        }
    }

    /// Whether MCP connectors may run: both switches must be on.
    pub fn mcp_active(&self) -> bool {
        self.dev_mode && self.mcp_enabled
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
            auto_suggest_tags: true,
            use_starter_tags: true,
            check_for_updates: true,
            spellcheck: true,
            spellcheck_language: SpellLanguage::British,
            document_languages: Vec::new(),
            last_update_check: None,
            update_mute: None,
            dev_mode: false,
            mcp_enabled: false,
            mcp_allow_edits: false,
            mcp_trusted_clients: Vec::new(),
            mcp_servers: Vec::new(),
            script_memory_mb: default_script_memory_mb(),
            script_total_memory_mb: default_script_total_memory_mb(),
        }
    }
}

impl Settings {
    pub fn to_toml(&self) -> Result<String, SettingsError> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn from_toml(text: &str) -> Result<Self, SettingsError> {
        let mut settings: Self = toml::from_str(text)?;
        // A hand-edited file must not switch MCP on behind a closed dev-mode gate.
        if !settings.dev_mode {
            settings.mcp_enabled = false;
        }
        if !settings.mcp_enabled {
            settings.mcp_allow_edits = false;
        }
        settings.mcp_servers.truncate(MAX_MCP_SERVERS);
        settings.script_memory_mb = crate::script::clamp_mb(settings.script_memory_mb);
        settings.script_total_memory_mb = crate::script::clamp_mb(settings.script_total_memory_mb);
        Ok(settings)
    }

    /// The language remembered for the file at `path`, if one was chosen.
    pub fn language_for(&self, path: &Path) -> Option<DocLanguage> {
        self.document_languages
            .iter()
            .find(|entry| entry.path == path)
            .map(|entry| entry.language)
    }

    /// The second language remembered for the file at `path`, if one was chosen.
    pub fn second_language_for(&self, path: &Path) -> Option<DocLanguage> {
        self.document_languages
            .iter()
            .find(|entry| entry.path == path)
            .and_then(|entry| entry.second)
    }

    /// Remembers `language` for the file at `path` (with no second language).
    #[cfg(test)]
    pub fn remember_language(&mut self, path: &Path, language: DocLanguage) -> bool {
        self.remember_languages(path, language, None)
    }

    /// Remembers a first and second language for the file at `path`: newest first, the oldest
    /// dropped past [`MAX_DOCUMENT_LANGUAGES`]. Returns whether anything changed.
    pub fn remember_languages(
        &mut self,
        path: &Path,
        language: DocLanguage,
        second: Option<DocLanguage>,
    ) -> bool {
        if self.document_languages.first().is_some_and(|entry| {
            entry.path == path && entry.language == language && entry.second == second
        }) {
            return false;
        }
        self.document_languages.retain(|entry| entry.path != path);
        self.document_languages.insert(
            0,
            DocumentLanguage {
                path: path.to_path_buf(),
                language,
                second,
            },
        );
        self.document_languages.truncate(MAX_DOCUMENT_LANGUAGES);
        true
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
    fn languages_are_remembered_by_path_newest_first_and_capped() {
        let mut settings = Settings::default();
        let a = Path::new("a.rtf");
        let b = Path::new("b.md");
        assert_eq!(settings.language_for(a), None);
        assert!(settings.remember_language(a, DocLanguage::American));
        assert!(settings.remember_language(b, DocLanguage::Korean));
        assert_eq!(settings.language_for(a), Some(DocLanguage::American));
        assert_eq!(settings.document_languages[0].path, b, "newest first");
        // The same again changes nothing; a new choice replaces the old one, once.
        assert!(!settings.remember_language(b, DocLanguage::Korean));
        assert!(settings.remember_language(a, DocLanguage::British));
        assert_eq!(settings.language_for(a), Some(DocLanguage::British));
        assert_eq!(settings.document_languages.len(), 2);
        for n in 0..MAX_DOCUMENT_LANGUAGES + 20 {
            settings.remember_language(Path::new(&format!("{n}.rtf")), DocLanguage::British);
        }
        assert_eq!(settings.document_languages.len(), MAX_DOCUMENT_LANGUAGES);
        assert_eq!(settings.language_for(a), None, "the oldest are forgotten");
        // An old settings file has no list.
        let old = Settings::from_toml("spellcheck = true").expect("parse");
        assert!(old.document_languages.is_empty());
    }

    #[test]
    fn a_sidebar_width_saved_by_an_older_version_is_widened() {
        let old = Settings::from_toml("sidebar_width = 260.0").expect("parse");
        assert_eq!(old.clamped_sidebar_width(), DEFAULT_SIDEBAR_WIDTH);
        let wide = Settings::from_toml("sidebar_width = 400.0").expect("parse");
        assert_eq!(wide.clamped_sidebar_width(), 400.0);
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
            auto_suggest_tags: false,
            use_starter_tags: false,
            check_for_updates: false,
            spellcheck: false,
            spellcheck_language: crate::spell::Language::American,
            document_languages: vec![DocumentLanguage {
                path: PathBuf::from("notes.rtf"),
                language: DocLanguage::Japanese,
                second: Some(DocLanguage::British),
            }],
            last_update_check: Some(1_700_000_000),
            update_mute: Some(UpdateMute {
                version: "0.1.3".to_owned(),
                until: Some(1_700_086_400),
            }),
            dev_mode: true,
            mcp_enabled: true,
            mcp_allow_edits: true,
            mcp_trusted_clients: vec!["Example Editor".to_owned()],
            mcp_servers: vec![McpServerConfig {
                name: "Notes".to_owned(),
                command_line: "npx -y some-server".to_owned(),
                approved_line: Some("npx -y some-server".to_owned()),
                token: Some("tok-123".to_owned()),
            }],
            script_memory_mb: 2048,
            script_total_memory_mb: 4096,
        };
        let text = original.to_toml().expect("serialise");
        assert_eq!(Settings::from_toml(&text).expect("parse"), original);
    }

    #[test]
    fn the_script_memory_cap_defaults_to_512mb_each_and_1gb_shared_and_is_clamped() {
        assert_eq!(Settings::default().script_memory_mb, 512);
        assert_eq!(Settings::default().script_total_memory_mb, 1024);
        // A file from before the setting existed gets the default.
        assert_eq!(
            Settings::from_toml("").expect("parse").script_memory_mb,
            512
        );
        let low = Settings::from_toml("script_memory_mb = 0").expect("parse");
        assert_eq!(low.script_memory_mb, crate::script::MIN_MEMORY_MB);
        let high = Settings::from_toml("script_memory_mb = 4000000000").expect("parse");
        assert_eq!(high.script_memory_mb, crate::script::MAX_MEMORY_MB);
        let total = Settings::from_toml("script_total_memory_mb = 1").expect("parse");
        assert_eq!(total.script_total_memory_mb, crate::script::MIN_MEMORY_MB);
    }

    #[test]
    fn a_token_belongs_to_a_web_address_and_stays_out_of_debug_output() {
        let mut s = Settings::default();
        let web = s
            .add_mcp_server("Web", "https://example.com/mcp")
            .expect("add");
        let program = s.add_mcp_server("Prog", "npx -y thing").expect("add");
        assert!(s.set_mcp_server_token(web, "  abc123  "));
        assert_eq!(s.mcp_servers[web].token.as_deref(), Some("abc123"));
        assert!(
            !s.set_mcp_server_token(program, "abc"),
            "a program takes none"
        );
        assert!(!s.set_mcp_server_token(web, "two words"));
        assert!(!s.set_mcp_server_token(web, &"a".repeat(MAX_MCP_TOKEN_CHARS + 1)));
        assert_eq!(
            s.mcp_servers[web].token.as_deref(),
            Some("abc123"),
            "kept on refusal"
        );
        assert!(!format!("{:?}", s.mcp_servers[web]).contains("abc123"));
        let text = s.to_toml().expect("serialise");
        assert_eq!(Settings::from_toml(&text).expect("parse"), s);
        assert!(!s.mcp_servers[program].token.is_some());
        assert!(s.set_mcp_server_token(web, ""), "empty clears");
        assert_eq!(s.mcp_servers[web].token, None);
        assert!(!s.to_toml().expect("serialise").contains("token"));
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
    fn developer_features_are_off_by_default() {
        let s = Settings::default();
        assert!(!s.dev_mode);
        assert!(!s.mcp_enabled);
        assert!(!s.mcp_active());
        let old = Settings::from_toml("theme = \"dark\"\n").expect("parse");
        assert!(!old.dev_mode && !old.mcp_enabled, "an old file has neither");
    }

    #[test]
    fn allow_edits_needs_mcp_and_goes_off_with_it() {
        let mut s = Settings::default();
        assert!(
            !s.mcp_allow_edits && !s.mcp_edits_allowed(),
            "off by default"
        );
        assert!(!s.set_mcp_allow_edits(true), "refused while MCP is off");
        s.set_dev_mode(true);
        assert!(!s.set_mcp_allow_edits(true), "dev mode alone is not enough");
        assert!(s.set_mcp_enabled(true));
        assert!(!s.mcp_edits_allowed(), "MCP on does not allow edits");
        assert!(s.set_mcp_allow_edits(true));
        assert!(s.mcp_edits_allowed());

        // Switching MCP off, or closing the dev-mode gate, switches editing off too.
        assert!(s.set_mcp_enabled(false));
        assert!(!s.mcp_allow_edits);
        assert!(s.set_mcp_enabled(true) && s.set_mcp_allow_edits(true));
        s.set_dev_mode(false);
        assert!(!s.mcp_allow_edits && !s.mcp_edits_allowed());
    }

    #[test]
    fn allow_edits_cannot_be_switched_on_by_editing_the_file() {
        let s = Settings::from_toml(
            "dev_mode = true
mcp_allow_edits = true
",
        )
        .expect("parse");
        assert!(!s.mcp_allow_edits, "MCP is off, so editing is forced off");
        let s = Settings::from_toml(
            "mcp_enabled = true
mcp_allow_edits = true
",
        )
        .expect("parse");
        assert!(!s.mcp_allow_edits, "dev mode is off, so everything is off");
        let s = Settings::from_toml(
            "dev_mode = true
mcp_enabled = true
mcp_allow_edits = true
",
        )
        .expect("parse");
        assert!(s.mcp_edits_allowed());
        let old = Settings::from_toml("").expect("parse");
        assert!(!old.mcp_allow_edits && old.mcp_trusted_clients.is_empty());
    }

    #[test]
    fn servers_are_added_trimmed_limited_and_start_unapproved() {
        let mut s = Settings::default();
        assert_eq!(s.add_mcp_server("  Notes ", " npx -y x  "), Some(0));
        assert_eq!(s.mcp_servers[0].name, "Notes");
        assert_eq!(s.mcp_servers[0].command_line, "npx -y x");
        assert!(!s.mcp_servers[0].is_approved());
        assert_eq!(s.add_mcp_server("", "x"), None);
        assert_eq!(s.add_mcp_server("x", "   "), None);
        assert_eq!(s.add_mcp_server(&"n".repeat(61), "x"), None);
        assert_eq!(s.add_mcp_server("n", &"c".repeat(501)), None);
        for i in 1..MAX_MCP_SERVERS {
            assert_eq!(s.add_mcp_server("n", "c"), Some(i));
        }
        assert_eq!(s.add_mcp_server("n", "c"), None, "the list is full");
    }

    #[test]
    fn approval_belongs_to_the_exact_command_line() {
        let mut s = Settings::default();
        let i = s.add_mcp_server("Notes", "npx -y x").expect("added");
        s.approve_mcp_server(i);
        assert!(s.mcp_servers[i].is_approved());
        let back = Settings::from_toml(&s.to_toml().expect("toml")).expect("parse");
        assert!(back.mcp_servers[i].is_approved(), "approval is saved");
        // Changing the command, even slightly, asks again.
        s.mcp_servers[i].command_line = "npx -y x --other".to_owned();
        assert!(!s.mcp_servers[i].is_approved());
        s.approve_mcp_server(i);
        assert!(s.mcp_servers[i].is_approved());
        s.approve_mcp_server(99);
        s.remove_mcp_server(99);
        s.remove_mcp_server(i);
        assert!(s.mcp_servers.is_empty());
        let old = Settings::from_toml("").expect("parse");
        assert!(old.mcp_servers.is_empty(), "an old file has no servers");
    }

    #[test]
    fn trusted_clients_are_remembered_once_saved_and_forgotten() {
        let mut s = Settings::default();
        assert!(!s.mcp_trusts("Example Editor"));
        s.trust_mcp_client("Example Editor");
        s.trust_mcp_client("Example Editor");
        assert_eq!(s.mcp_trusted_clients, ["Example Editor"]);
        let back = Settings::from_toml(&s.to_toml().expect("toml")).expect("parse");
        assert!(back.mcp_trusts("Example Editor"));
        assert!(!back.mcp_trusts("Someone else"));
        s.forget_mcp_clients();
        assert!(!s.mcp_trusts("Example Editor"));
    }

    #[test]
    fn mcp_needs_dev_mode() {
        let mut s = Settings::default();
        assert!(!s.set_mcp_enabled(true), "refused while dev mode is off");
        assert!(!s.mcp_enabled);

        s.set_dev_mode(true);
        assert!(!s.mcp_active(), "dev mode alone does not start MCP");
        assert!(s.set_mcp_enabled(true));
        assert!(s.mcp_active());

        s.set_dev_mode(false);
        assert!(!s.mcp_enabled, "closing the gate switches MCP off");
        assert!(!s.mcp_active());
        assert!(s.set_mcp_enabled(false), "turning it off is always allowed");
    }

    #[test]
    fn a_hand_edited_file_cannot_open_the_gate() {
        let s = Settings::from_toml("mcp_enabled = true\n").expect("parse");
        assert!(!s.mcp_enabled, "dev mode is off, so MCP is forced off");
        assert!(!s.mcp_active());

        let s = Settings::from_toml("dev_mode = true\nmcp_enabled = true\n").expect("parse");
        assert!(s.mcp_active());
    }

    #[test]
    fn developer_switches_round_trip() {
        let mut s = Settings::default();
        s.set_dev_mode(true);
        assert!(s.set_mcp_enabled(true));
        let text = s.to_toml().expect("serialise");
        assert!(text.contains("dev_mode = true"), "{text}");
        assert!(text.contains("mcp_enabled = true"), "{text}");
        assert_eq!(Settings::from_toml(&text).expect("parse"), s);
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
