//! The Characters tab's Import pop-up: choose the story's fandoms, agree (once) to look online,
//! fetch character names from wikis or type them in, and pick which to save.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use iced::widget::{
    button, column, container, mouse_area, row, scrollable, space, stack, text, text_input,
};
use iced::{Element, Length, Task, border};

use libertum_model::Characters;
use libertum_wiki::{Candidate, Request, Site};

use super::{App, Message, Modal};
use crate::inspector::characters::MAX_SHOWN;
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::{self, checkbox_row, flat};
use crate::wiki_fetch::{self, Found, MAX_REQUESTS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Choosing the story's fandoms.
    Fandoms,
    /// The one-time question about looking online.
    Consent,
    /// Choosing where to look.
    Sources,
    Fetching,
    /// Choosing which names to save.
    Pick,
    /// Typing or pasting names.
    Offline,
}

/// Everything the pop-up holds. Nothing is saved to the document until Add.
#[derive(Clone, Debug)]
pub struct ImportState {
    pub stage: Stage,
    pub fandom_input: String,
    pub fandoms: Vec<String>,
    pub wiki_name: String,
    pub categories: String,
    pub site_input: String,
    pub site_error: Option<&'static str>,
    pub found: Vec<Candidate>,
    /// What AO3's recent works said about each fandom looked up (as typed, then the profile).
    pub profiles: Vec<(String, libertum_wiki::Profile)>,
    pub report: Vec<String>,
    /// Indices into `found`.
    pub picked: HashSet<usize>,
    pub search: String,
    pub paste: String,
    /// The names being picked came from typing, not a lookup, so Back returns to typing.
    pub typed: bool,
    pub cancel: Arc<AtomicBool>,
}

impl ImportState {
    fn new(fandoms: Vec<String>) -> Self {
        let wiki_name = fandoms
            .first()
            .and_then(|fandom| libertum_wiki::slug(fandom))
            .unwrap_or_default();
        Self {
            stage: Stage::Fandoms,
            fandom_input: String::new(),
            fandoms,
            wiki_name,
            categories: "Characters".to_owned(),
            site_input: String::new(),
            site_error: None,
            found: Vec::new(),
            profiles: Vec::new(),
            report: Vec::new(),
            picked: HashSet::new(),
            search: String::new(),
            paste: String::new(),
            typed: false,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The names to show: the ones not saved yet that match the search, at most
    /// [`MAX_SHOWN`], and the number that matched. When the story has exactly one fandom only
    /// names found for that fandom are ever shown, so a crossover or another fandom's characters
    /// cannot leak into the list.
    pub fn visible(&self, cast: &Characters) -> (Vec<usize>, usize) {
        let query = self.search.trim().to_lowercase();
        let only = (self.fandoms.len() == 1).then(|| self.fandoms[0].to_lowercase());
        let matching: Vec<usize> = self
            .found
            .iter()
            .enumerate()
            .filter(|(_, c)| only.as_ref().is_none_or(|f| c.fandom.to_lowercase() == *f))
            .filter(|(_, c)| !cast.has_name(&c.name, None))
            .filter(|(_, c)| query.is_empty() || c.name.to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect();
        let total = matching.len();
        (matching.into_iter().take(MAX_SHOWN).collect(), total)
    }
}

/// Names typed, pasted or read from a file: separated by semicolons or line breaks, or by commas
/// when there is nothing else. Spaces are tidied and repeats dropped.
pub fn parse_typed_names(text: &str, fandom: &str) -> Vec<Candidate> {
    let mut parts: Vec<&str> = text
        .split(['\n', ';'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() == 1 {
        parts = parts[0]
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for part in parts {
        let name = part.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.chars().count() <= 80 && seen.insert(name.to_lowercase()) {
            out.push(Candidate {
                name: name.clone(),
                fandom: fandom.to_owned(),
                site: String::new(),
                title: name,
                uses: 0,
            });
        }
    }
    out
}

#[derive(Clone, Debug)]
pub enum ImportMsg {
    Open,
    FandomInput(String),
    FandomAdd,
    FandomRemove(usize),
    FandomsDone,
    Consent(bool),
    WikiName(String),
    Categories(String),
    SiteToggle(String),
    SiteInput(String),
    SiteAdd,
    SiteRemove(String),
    Fetch,
    Fetched(Found),
    Stop,
    Close,
    Search(String),
    Pick(usize),
    PickShown,
    PickNone,
    Add,
    Paste(String),
    UseNames,
    FromFile,
    FileRead(Result<String, String>),
    AllowOnline,
    Back,
    TypeInstead,
}

impl App {
    fn import_state(&mut self) -> Option<&mut ImportState> {
        match &mut self.modal {
            Some(Modal::CharacterImport(state)) => Some(state),
            _ => None,
        }
    }

    /// Where to go once the fandoms are known: ask about looking online the first time.
    fn import_stage_after_fandoms(&self) -> Stage {
        match self.settings.wiki_consent {
            None => Stage::Consent,
            Some(true) => Stage::Sources,
            Some(false) => Stage::Offline,
        }
    }

    /// The sites to look in, in order and without the ones switched off: Fandom first, then the
    /// writer's own, then the rest.
    pub(super) fn wiki_sites(&self) -> Vec<Site> {
        // AO3 first, then Fandom, then the writer's own, then the rest.
        let mut built = libertum_wiki::built_in();
        let mut ordered = vec![built.remove(0), built.remove(0)];
        ordered.extend(self.settings.wiki_sites.iter().cloned());
        ordered.extend(built);
        ordered
    }

    pub(super) fn wiki_sites_on(&self) -> Vec<Site> {
        self.wiki_sites()
            .into_iter()
            .filter(|site| !self.settings.wiki_sites_off.contains(&site.id))
            // Wikis only: AO3 is never contacted.
            .filter(|site| self.settings.ao3_first || site.kind != libertum_wiki::SiteKind::Ao3)
            .collect()
    }

    pub(super) fn on_import(&mut self, message: ImportMsg) -> Task<Message> {
        match message {
            ImportMsg::Open => {
                let fandoms = self.characters.list.fandoms().to_vec();
                let mut state = ImportState::new(fandoms.clone());
                if !fandoms.is_empty() {
                    state.stage = self.import_stage_after_fandoms();
                }
                self.modal = Some(Modal::CharacterImport(state));
            }
            ImportMsg::FandomInput(text) => {
                if let Some(state) = self.import_state() {
                    state.fandom_input = text;
                }
            }
            ImportMsg::FandomAdd => {
                if let Some(state) = self.import_state() {
                    add_fandom(state);
                }
            }
            ImportMsg::FandomRemove(index) => {
                if let Some(state) = self.import_state()
                    && index < state.fandoms.len()
                {
                    state.fandoms.remove(index);
                }
            }
            ImportMsg::FandomsDone => {
                let Some(state) = self.import_state() else {
                    return Task::none();
                };
                add_fandom(state);
                if state.fandoms.is_empty() {
                    return Task::none();
                }
                let fandoms = state.fandoms.clone();
                state.wiki_name = libertum_wiki::slug(&fandoms[0]).unwrap_or_default();
                self.characters.list.set_fandoms(fandoms);
                self.characters_changed();
                let next = self.import_stage_after_fandoms();
                if let Some(state) = self.import_state() {
                    state.stage = next;
                }
            }
            ImportMsg::Consent(agreed) => {
                self.settings.wiki_consent = Some(agreed);
                self.save_settings();
                if let Some(state) = self.import_state() {
                    state.stage = if agreed {
                        Stage::Sources
                    } else {
                        Stage::Offline
                    };
                }
            }
            ImportMsg::AllowOnline => {
                if let Some(state) = self.import_state() {
                    state.stage = Stage::Consent;
                }
            }
            ImportMsg::TypeInstead => {
                if let Some(state) = self.import_state() {
                    state.stage = Stage::Offline;
                }
            }
            ImportMsg::WikiName(text) => {
                if let Some(state) = self.import_state() {
                    state.wiki_name = text;
                }
            }
            ImportMsg::Categories(text) => {
                if let Some(state) = self.import_state() {
                    state.categories = text;
                }
            }
            ImportMsg::SiteToggle(id) if id == "ao3" => {
                // The same choice as in Settings.
                self.settings.ao3_first = !self.settings.ao3_first;
                self.save_settings();
            }
            ImportMsg::SiteToggle(id) => {
                if let Some(position) = self.settings.wiki_sites_off.iter().position(|o| *o == id) {
                    self.settings.wiki_sites_off.remove(position);
                } else {
                    self.settings.wiki_sites_off.push(id);
                }
                self.save_settings();
            }
            ImportMsg::SiteInput(text) => {
                if let Some(state) = self.import_state() {
                    state.site_input = text;
                    state.site_error = None;
                }
            }
            ImportMsg::SiteAdd => {
                let Some(state) = self.import_state() else {
                    return Task::none();
                };
                let input = state.site_input.clone();
                let parsed = libertum_wiki::parse_custom(&input);
                let known: Vec<String> =
                    self.wiki_sites().into_iter().map(|site| site.id).collect();
                let result = match parsed {
                    Err(libertum_wiki::SiteError::NotHttps) => Err(s::IMPORT_SITE_NOT_HTTPS),
                    Err(libertum_wiki::SiteError::BadAddress) => Err(s::IMPORT_SITE_BAD),
                    Err(libertum_wiki::SiteError::Private) => Err(s::IMPORT_SITE_PRIVATE),
                    Ok(site) if known.contains(&site.id) => Err(s::IMPORT_SITE_HAVE),
                    Ok(site) => Ok(site),
                };
                match result {
                    Ok(site) => {
                        self.settings.wiki_sites.push(site);
                        self.save_settings();
                        if let Some(state) = self.import_state() {
                            state.site_input.clear();
                            state.site_error = None;
                        }
                    }
                    Err(error) => {
                        if let Some(state) = self.import_state() {
                            state.site_error = Some(error);
                        }
                    }
                }
            }
            ImportMsg::SiteRemove(id) => {
                self.settings.wiki_sites.retain(|site| site.id != id);
                self.settings.wiki_sites_off.retain(|off| *off != id);
                self.save_settings();
            }
            ImportMsg::Fetch => {
                // Nothing goes out unless the writer has agreed, and only from this button.
                if self.settings.wiki_consent != Some(true) {
                    if let Some(state) = self.import_state() {
                        state.stage = Stage::Consent;
                    }
                    return Task::none();
                }
                let sites = self.wiki_sites_on();
                let Some(state) = self.import_state() else {
                    return Task::none();
                };
                if state.fandoms.is_empty() {
                    state.stage = Stage::Fandoms;
                    return Task::none();
                }
                let requests = fetch_requests(state, &sites);
                state.cancel = Arc::new(AtomicBool::new(false));
                state.stage = Stage::Fetching;
                state.report.clear();
                let cancel = state.cancel.clone();
                return Task::perform(async move { wiki_fetch::find(requests, cancel) }, |found| {
                    Message::Import(ImportMsg::Fetched(found))
                });
            }
            ImportMsg::Fetched(found) => {
                if let Some(state) = self.import_state()
                    && state.stage == Stage::Fetching
                {
                    state.typed = false;
                    state.picked.clear();
                    state.search.clear();
                    state.stage = if found.candidates.is_empty() {
                        Stage::Sources
                    } else {
                        Stage::Pick
                    };
                    state.report = found.report;
                    if found.candidates.is_empty() && state.report.is_empty() {
                        state.report.push(s::IMPORT_NOTHING_FOUND.to_owned());
                    }
                    let mut candidates = found.candidates;
                    // AO3 is the source of truth for the fandom's name: use its spelling.
                    if state.fandoms.len() == 1
                        && let Some((_, profile)) = found.profiles.first()
                        && !profile.fandom.is_empty()
                        && !profile.fandom.eq_ignore_ascii_case(&state.fandoms[0])
                    {
                        let old = std::mem::replace(&mut state.fandoms[0], profile.fandom.clone());
                        for candidate in &mut candidates {
                            if candidate.fandom == old {
                                candidate.fandom = profile.fandom.clone();
                            }
                        }
                        state
                            .report
                            .push(format!("{} {}", s::IMPORT_AO3_NAME, profile.fandom));
                    }
                    if !found.profiles.is_empty() {
                        state.report.push(s::IMPORT_TAGS_SAVED.to_owned());
                    }
                    state.profiles = found.profiles;
                    state.found = candidates;
                }
            }
            ImportMsg::Stop => {
                if let Some(state) = self.import_state() {
                    state.cancel.store(true, Ordering::Relaxed);
                    state.stage = Stage::Sources;
                }
            }
            ImportMsg::Close => {
                if let Some(state) = self.import_state() {
                    state.cancel.store(true, Ordering::Relaxed);
                }
                if matches!(self.modal, Some(Modal::CharacterImport(_))) {
                    self.modal = None;
                }
            }
            ImportMsg::Search(text) => {
                if let Some(state) = self.import_state() {
                    state.search = text;
                }
            }
            ImportMsg::Pick(index) => {
                if let Some(state) = self.import_state()
                    && !state.picked.remove(&index)
                {
                    state.picked.insert(index);
                }
            }
            ImportMsg::PickShown => {
                let cast = self.characters.list.clone();
                if let Some(state) = self.import_state() {
                    let (shown, _) = state.visible(&cast);
                    state.picked.extend(shown);
                }
            }
            ImportMsg::PickNone => {
                if let Some(state) = self.import_state() {
                    state.picked.clear();
                }
            }
            ImportMsg::Add => {
                let Some(Modal::CharacterImport(state)) = &self.modal else {
                    return Task::none();
                };
                let mut names: Vec<String> = state
                    .picked
                    .iter()
                    .filter_map(|index| state.found.get(*index))
                    .map(|candidate| candidate.name.clone())
                    .collect();
                names.sort_by_key(|name| name.to_lowercase());
                let fandoms = state.fandoms.clone();
                let profiles = state.profiles.clone();
                // AO3's spelling of the fandom replaces the typed one, then the tags read for it
                // are kept for the tag suggestions.
                self.characters.list.set_fandoms(fandoms.clone());
                for (typed, profile) in &profiles {
                    let fandom = if fandoms.len() == 1 {
                        fandoms[0].clone()
                    } else {
                        typed.clone()
                    };
                    let tags = fandom_tags(&fandom, profile);
                    self.characters.list.set_fandom_tags(&fandom, tags);
                }
                for name in &names {
                    self.characters.list.add_name(name);
                }
                self.modal = None;
                self.characters_changed();
            }
            ImportMsg::Paste(text) => {
                if let Some(state) = self.import_state() {
                    state.paste = text;
                }
            }
            ImportMsg::UseNames => {
                if let Some(state) = self.import_state() {
                    let text = state.paste.clone();
                    use_typed(state, &text);
                }
            }
            ImportMsg::FromFile => {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter(s::IMPORT_FILE_FILTER, &["txt", "csv", "list"])
                    .set_title(s::IMPORT_FILE_TITLE)
                    .pick_file()
                else {
                    return Task::none();
                };
                return Task::perform(
                    async move {
                        // A names list is small; anything big is not one.
                        match std::fs::metadata(&path) {
                            Ok(meta) if meta.len() > 1_000_000 => Err("too large".to_owned()),
                            Ok(_) => std::fs::read_to_string(&path).map_err(|e| e.to_string()),
                            Err(error) => Err(error.to_string()),
                        }
                    },
                    |result| Message::Import(ImportMsg::FileRead(result)),
                );
            }
            ImportMsg::FileRead(result) => {
                if let Some(state) = self.import_state()
                    && let Ok(text) = result
                {
                    use_typed(state, &text);
                }
            }
            ImportMsg::Back => {
                if let Some(state) = self.import_state() {
                    state.stage = if state.typed {
                        Stage::Offline
                    } else {
                        Stage::Sources
                    };
                }
            }
        }
        Task::none()
    }
}

/// The relationship and additional tags of an AO3 profile, for the tag suggestions.
fn fandom_tags(fandom: &str, profile: &libertum_wiki::Profile) -> Vec<libertum_model::FandomTag> {
    use libertum_model::{FandomTag, FandomTagKind};
    let make = |kind, tags: &[libertum_wiki::Tagged]| -> Vec<FandomTag> {
        tags.iter()
            .map(|tag| FandomTag {
                name: tag.name.clone(),
                kind,
                uses: tag.uses,
                fandom: fandom.to_owned(),
            })
            .collect()
    };
    let mut out = make(FandomTagKind::Relationship, &profile.relationships);
    out.extend(make(FandomTagKind::Freeform, &profile.freeforms));
    out
}

fn add_fandom(state: &mut ImportState) {
    let name = state
        .fandom_input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    state.fandom_input.clear();
    if !name.is_empty() && !state.fandoms.iter().any(|f| f.eq_ignore_ascii_case(&name)) {
        state.fandoms.push(name);
    }
}

fn use_typed(state: &mut ImportState, text: &str) {
    let fandom = state.fandoms.first().cloned().unwrap_or_default();
    state.found = parse_typed_names(text, &fandom);
    state.picked.clear();
    state.search.clear();
    state.report.clear();
    state.typed = true;
    state.stage = Stage::Pick;
}

/// One request per fandom. A single fandom may use the wiki name the writer typed; with several,
/// each fandom's own name is used.
fn fetch_requests(state: &ImportState, sites: &[Site]) -> Vec<Request> {
    let typed: Vec<String> = state
        .categories
        .split(',')
        .map(|c| c.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|c| !c.is_empty())
        .collect();
    state
        .fandoms
        .iter()
        .map(|fandom| {
            let slug = if state.fandoms.len() == 1 {
                libertum_wiki::slug(&state.wiki_name).unwrap_or_default()
            } else {
                libertum_wiki::slug(fandom).unwrap_or_default()
            };
            // Wikis name their character category differently: "Characters" on most, and
            // "Individuals" or "Humans" on others (Harry Potter's and Star Wars' Fandom wikis).
            let mut categories = if typed.is_empty() {
                ["Characters", "Individuals", "Humans"]
                    .map(str::to_owned)
                    .to_vec()
            } else {
                typed.clone()
            };
            // Wikipedia and some wikis name the category after the fandom.
            categories.push(format!("{fandom} characters"));
            Request {
                fandom: fandom.clone(),
                slug,
                sites: sites.to_vec(),
                categories,
                max_requests: MAX_REQUESTS,
            }
        })
        .collect()
}

// ------------------------------------------------------------------------------- view

fn primary<'a>(
    t: Tokens,
    label: impl Into<String>,
    message: Option<Message>,
) -> iced::widget::Button<'a, Message> {
    button(text(label.into()).size(13))
        .padding([6, 18])
        .style(move |_, status| {
            let on = !matches!(status, button::Status::Disabled);
            button::Style {
                background: Some(if on { t.accent } else { t.control }.into()),
                text_color: if on { t.on_accent } else { t.text_secondary },
                border: border::rounded(6),
                ..button::Style::default()
            }
        })
        .on_press_maybe(message)
}

fn quiet<'a>(t: Tokens, label: &'a str, message: Message) -> iced::widget::Button<'a, Message> {
    button(text(label).size(13))
        .padding([6, 16])
        .style(move |_, status| flat(t, status))
        .on_press(message)
}

fn link<'a>(t: Tokens, label: &'a str, message: Message) -> iced::widget::Button<'a, Message> {
    button(text(label).size(12).color(t.accent))
        .padding(0)
        .style(button::text)
        .on_press(message)
}

fn note<'a>(t: Tokens, content: impl Into<String>) -> iced::widget::Text<'a> {
    text(content.into()).size(12).color(t.text_secondary)
}

impl App {
    pub(super) fn character_import_layer<'a>(
        &'a self,
        state: &'a ImportState,
        t: Tokens,
    ) -> Element<'a, Message> {
        let import = Message::Import;
        let mut body = column![].spacing(12);
        let mut title = s::IMPORT_TITLE;
        let mut buttons = row![space::horizontal()].spacing(8);

        match state.stage {
            Stage::Fandoms => {
                title = s::IMPORT_FANDOM_HEADING;
                let mut chips = column![].spacing(4);
                for (index, fandom) in state.fandoms.iter().enumerate() {
                    chips = chips.push(
                        row![
                            text(fandom.as_str()).size(13).color(t.text),
                            space::horizontal(),
                            link(
                                t,
                                s::IMPORT_SITE_REMOVE,
                                import(ImportMsg::FandomRemove(index))
                            ),
                        ]
                        .spacing(8),
                    );
                }
                let input = text_input(s::IMPORT_FANDOM_PLACEHOLDER, &state.fandom_input)
                    .on_input(move |text| import(ImportMsg::FandomInput(text)))
                    .on_submit(import(ImportMsg::FandomAdd))
                    .size(13)
                    .padding(8)
                    .width(Length::Fill);
                let add = quiet(t, s::CHARACTERS_ADD, import(ImportMsg::FandomAdd));
                body = body
                    .push(note(t, s::IMPORT_FANDOM_HINT))
                    .push(row![input, add].spacing(6))
                    .push(chips);
                let ready = !state.fandoms.is_empty() || !state.fandom_input.trim().is_empty();
                buttons = buttons
                    .push(quiet(t, s::CANCEL, import(ImportMsg::Close)))
                    .push(primary(
                        t,
                        s::IMPORT_NEXT,
                        ready.then(|| import(ImportMsg::FandomsDone)),
                    ));
            }
            Stage::Consent => {
                title = s::IMPORT_CONSENT_TITLE;
                body = body.push(text(s::IMPORT_CONSENT_BODY).size(13).color(t.text));
                buttons = buttons
                    .push(quiet(
                        t,
                        s::IMPORT_CONSENT_NO,
                        import(ImportMsg::Consent(false)),
                    ))
                    .push(primary(
                        t,
                        s::IMPORT_CONSENT_YES,
                        Some(import(ImportMsg::Consent(true))),
                    ));
            }
            Stage::Sources => {
                body = body.push(
                    text(state.fandoms.join(", "))
                        .size(13)
                        .font(controls::bold())
                        .color(t.text),
                );
                body = body.push(
                    text(s::IMPORT_SITES_HEADING)
                        .size(12)
                        .color(t.text_secondary),
                );
                let mut sites = column![].spacing(4);
                for site in self.wiki_sites() {
                    let id = site.id.clone();
                    let on = if site.kind == libertum_wiki::SiteKind::Ao3 {
                        self.settings.ao3_first
                    } else {
                        !self.settings.wiki_sites_off.contains(&site.id)
                    };
                    let toggle_id = id.clone();
                    let mut line = row![checkbox_row(t, &site.name, on, false, move |_| {
                        import(ImportMsg::SiteToggle(toggle_id.clone()))
                    })]
                    .spacing(8);
                    if site.kind == libertum_wiki::SiteKind::Custom {
                        line = line.push(space::horizontal());
                        line = line.push(link(
                            t,
                            s::IMPORT_SITE_REMOVE,
                            import(ImportMsg::SiteRemove(id)),
                        ));
                    }
                    sites = sites.push(line);
                }
                body = body.push(sites);
                let site_input = text_input(s::IMPORT_SITE_PLACEHOLDER, &state.site_input)
                    .on_input(move |text| import(ImportMsg::SiteInput(text)))
                    .on_submit(import(ImportMsg::SiteAdd))
                    .size(12)
                    .padding(6)
                    .width(Length::Fill);
                body = body.push(
                    row![
                        site_input,
                        quiet(t, s::IMPORT_SITE_ADD, import(ImportMsg::SiteAdd))
                    ]
                    .spacing(6),
                );
                if let Some(error) = state.site_error {
                    body = body.push(note(t, error));
                }
                if state.fandoms.len() == 1 {
                    body = body.push(note(t, s::IMPORT_WIKI_NAME)).push(
                        text_input("", &state.wiki_name)
                            .on_input(move |text| import(ImportMsg::WikiName(text)))
                            .size(12)
                            .padding(6),
                    );
                }
                body = body.push(note(t, s::IMPORT_CATEGORIES)).push(
                    text_input("Characters", &state.categories)
                        .on_input(move |text| import(ImportMsg::Categories(text)))
                        .on_submit(import(ImportMsg::Fetch))
                        .size(12)
                        .padding(6),
                );
                for line in &state.report {
                    body = body.push(note(t, line.as_str()));
                }
                buttons = row![
                    link(t, s::IMPORT_TYPE_INSTEAD, import(ImportMsg::TypeInstead)),
                    space::horizontal(),
                    quiet(t, s::CANCEL, import(ImportMsg::Close)),
                    primary(t, s::IMPORT_FETCH, Some(import(ImportMsg::Fetch))),
                ]
                .spacing(8);
            }
            Stage::Fetching => {
                body = body.push(text(s::IMPORT_FETCHING).size(13).color(t.text));
                buttons = buttons.push(quiet(t, s::IMPORT_STOP, import(ImportMsg::Stop)));
            }
            Stage::Offline => {
                title = s::IMPORT_OFFLINE_HEADING;
                body = body.push(note(t, s::IMPORT_OFFLINE_HINT)).push(
                    text_input(s::IMPORT_OFFLINE_PLACEHOLDER, &state.paste)
                        .on_input(move |text| import(ImportMsg::Paste(text)))
                        .on_submit(import(ImportMsg::UseNames))
                        .size(13)
                        .padding(8),
                );
                let mut left =
                    row![link(t, s::IMPORT_FROM_FILE, import(ImportMsg::FromFile))].spacing(12);
                if self.settings.wiki_consent != Some(true) {
                    left = left.push(link(
                        t,
                        s::IMPORT_ALLOW_ONLINE,
                        import(ImportMsg::AllowOnline),
                    ));
                } else {
                    left = left.push(link(t, s::IMPORT_BACK, import(ImportMsg::Back)));
                }
                let ready = !state.paste.trim().is_empty();
                buttons = row![
                    left,
                    space::horizontal(),
                    quiet(t, s::CANCEL, import(ImportMsg::Close)),
                    primary(
                        t,
                        s::IMPORT_USE_NAMES,
                        ready.then(|| import(ImportMsg::UseNames))
                    ),
                ]
                .spacing(8);
            }
            Stage::Pick => {
                let (shown, total) = state.visible(&self.characters.list);
                let origin = state
                    .found
                    .first()
                    .map(|c| c.site.as_str())
                    .filter(|site| !site.is_empty());
                let mut heading = format!("{} {}", state.found.len(), s::IMPORT_FOUND);
                if let Some(site) = origin {
                    heading = format!("{heading} {} {site}", s::IMPORT_FROM);
                }
                body = body.push(text(heading).size(13).color(t.text));
                if origin == Some("AO3") {
                    body = body.push(note(t, s::IMPORT_AO3_NOTE));
                }
                for line in &state.report {
                    body = body.push(note(t, line.as_str()));
                }
                body = body.push(
                    text_input(s::IMPORT_SEARCH, &state.search)
                        .on_input(move |text| import(ImportMsg::Search(text)))
                        .size(12)
                        .padding(6),
                );
                let mut list = column![].spacing(2);
                for index in &shown {
                    let index = *index;
                    let candidate = &state.found[index];
                    let label = if candidate.uses > 0 {
                        format!("{} \u{00b7} {}", candidate.name, candidate.uses)
                    } else {
                        candidate.name.clone()
                    };
                    list = list.push(checkbox_row(
                        t,
                        &label,
                        state.picked.contains(&index),
                        false,
                        move |_| import(ImportMsg::Pick(index)),
                    ));
                }
                if shown.is_empty() {
                    list = list.push(note(t, s::IMPORT_NONE_LEFT));
                }
                // Tall enough for ten names; the rest scroll.
                body = body.push(scrollable(list).height(Length::Fixed(206.0)).spacing(4));
                if total > shown.len() {
                    body = body.push(note(
                        t,
                        format!(
                            "{} {} {} {total}. {}",
                            s::CHARACTERS_SHOWING,
                            shown.len(),
                            s::CHARACTERS_OF,
                            s::CHARACTERS_SEARCH_FOR_MORE
                        ),
                    ));
                }
                body = body.push(
                    row![
                        link(t, s::IMPORT_SELECT_SHOWN, import(ImportMsg::PickShown)),
                        link(t, s::IMPORT_CLEAR, import(ImportMsg::PickNone)),
                    ]
                    .spacing(14),
                );
                let count = state.picked.len();
                buttons = row![
                    link(t, s::IMPORT_BACK, import(ImportMsg::Back)),
                    space::horizontal(),
                    quiet(t, s::CANCEL, import(ImportMsg::Close)),
                    primary(
                        t,
                        format!("{} ({count})", s::IMPORT_ADD_SELECTED),
                        (count > 0).then(|| import(ImportMsg::Add)),
                    ),
                ]
                .spacing(8);
            }
        }

        let card = container(
            column![
                text(title).size(15).font(controls::bold()).color(t.text),
                body,
                buttons,
            ]
            .spacing(14),
        )
        .padding(18)
        .width(Length::Fixed(480.0))
        .style(move |_| container::Style {
            background: Some(t.inspector.into()),
            border: border::rounded(12),
            shadow: iced::Shadow {
                color: t.shadow,
                offset: iced::Vector::new(0.0, 8.0),
                blur_radius: 24.0,
            },
            ..container::Style::default()
        });
        stack![
            mouse_area(space().width(Length::Fill).height(Length::Fill))
                .on_press(Message::Import(ImportMsg::Close)),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(names: &[(&str, &str)]) -> Vec<Candidate> {
        names
            .iter()
            .map(|(name, fandom)| Candidate {
                name: (*name).to_owned(),
                fandom: (*fandom).to_owned(),
                site: "Fandom".to_owned(),
                title: (*name).to_owned(),
                uses: 0,
            })
            .collect()
    }

    #[test]
    fn typed_names_split_on_semicolons_lines_or_commas_and_drop_repeats() {
        let names = |text: &str| -> Vec<String> {
            parse_typed_names(text, "X")
                .into_iter()
                .map(|c| c.name)
                .collect()
        };
        assert_eq!(
            names("Mara Voss; Jo  Ellery;mara voss"),
            ["Mara Voss", "Jo Ellery"]
        );
        assert_eq!(names("Mara Voss\nJo Ellery\n"), ["Mara Voss", "Jo Ellery"]);
        assert_eq!(names("Mara, Jo, Al"), ["Mara", "Jo", "Al"]);
        assert_eq!(
            names("Voss, Mara; Jo"),
            ["Voss, Mara", "Jo"],
            "a semicolon list keeps its commas"
        );
        assert!(names("  ; ").is_empty());
    }

    #[test]
    fn the_list_shows_fifty_hides_saved_names_and_a_single_fandom_sees_only_its_own() {
        let mut state = ImportState::new(vec!["Harry Potter".into()]);
        let mut all: Vec<(String, &str)> = (0..80)
            .map(|i| (format!("Person {i:02}"), "Harry Potter"))
            .collect();
        all.push(("Crossover Carl".into(), "Star Wars"));
        let pairs: Vec<(&str, &str)> = all.iter().map(|(n, f)| (n.as_str(), *f)).collect();
        state.found = found(&pairs);
        let mut cast = Characters::default();
        cast.add("person 00", "", "", vec![]).unwrap();

        let (shown, total) = state.visible(&cast);
        assert_eq!(shown.len(), MAX_SHOWN, "at most fifty at once");
        assert_eq!(
            total, 79,
            "80 of the fandom, less the saved one; the crossover never counts"
        );
        assert!(
            shown
                .iter()
                .all(|i| state.found[*i].fandom == "Harry Potter")
        );
        assert!(!shown.iter().any(|i| state.found[*i].name == "Person 00"));

        state.search = "PERSON 7".into();
        let (shown, total) = state.visible(&cast);
        assert_eq!((shown.len(), total), (10, 10));

        // Two fandoms: both are shown.
        state.fandoms.push("Star Wars".into());
        state.search.clear();
        assert_eq!(state.visible(&cast).1, 80);
    }
}
