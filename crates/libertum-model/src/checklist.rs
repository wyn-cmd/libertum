//! The story checklist: points the writer wants each chapter to make, scored against the
//! words of the chapter being written.
//!
//! An item is a line of text plus optional extra words (aliases). Its *keywords* are the
//! meaningful words of both, reduced to a rough stem so plural and tense forms of regular words ("steal", "steals", "stealing") meet.
//! A chapter is summarised once as a [`ChapterProfile`] (how often each stem occurs); scoring
//! an item against it is then a handful of hash lookups, so the list can be re-ranked on
//! every edit.
//!
//! Coverage is a number from 0 to 1: how much of the item's keywords the chapter already
//! uses. One mention of a word counts for half of it, two for three quarters, and so on, so
//! a word repeated forty times does not outweigh a whole missing idea. This is word matching,
//! not understanding: it can only say the chapter has not used the item's words.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// How much an alias counts against an item's own words.
const ALIAS_WEIGHT: f32 = 0.5;

/// Words too common to say anything about a point.
const STOP_WORDS: &[&str] = &[
    "a", "about", "after", "again", "all", "also", "an", "and", "any", "are", "as", "at", "be",
    "because", "been", "before", "being", "but", "by", "can", "could", "did", "do", "does", "for",
    "from", "had", "has", "have", "he", "her", "here", "him", "his", "how", "i", "if", "in",
    "into", "is", "it", "its", "just", "me", "more", "most", "my", "no", "not", "of", "on", "one",
    "or", "our", "out", "over", "she", "should", "so", "some", "than", "that", "the", "their",
    "them", "then", "there", "these", "they", "this", "those", "to", "up", "us", "was", "we",
    "were", "what", "when", "where", "which", "while", "who", "why", "will", "with", "would",
    "you", "your",
];

/// A chapter named by its position and heading, the same pairing the "last posted" copies use:
/// a position alone follows the wrong chapter when one is inserted, a title alone cannot tell
/// apart two chapters called "Interlude".
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChapterRef {
    pub index: usize,
    pub title: String,
}

impl ChapterRef {
    pub fn new(index: usize, title: impl Into<String>) -> Self {
        Self {
            index,
            title: title.into(),
        }
    }
}

/// Which chapters a point appears in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    /// Every chapter of the story.
    #[default]
    Story,
    /// Only the listed chapters, and with `onward` every chapter after the last of them.
    /// A point for one chapter is this with that chapter alone and `onward` off.
    Chapters {
        chapters: Vec<ChapterRef>,
        onward: bool,
    },
}

impl Scope {
    /// Only `chapter`, nowhere else.
    pub fn only(chapter: ChapterRef) -> Self {
        Self::Chapters {
            chapters: vec![chapter],
            onward: false,
        }
    }

    /// Whether a point with this scope appears in `chapter`.
    pub fn includes(&self, chapter: &ChapterRef) -> bool {
        match self {
            Self::Story => true,
            Self::Chapters { chapters, onward } => {
                refers_to(chapters, chapter)
                    || (*onward
                        && chapters
                            .iter()
                            .map(|c| c.index)
                            .max()
                            .is_some_and(|last| chapter.index > last))
            }
        }
    }
}

/// Whether `chapter` is one of `list`: an exact position-and-title match, else the one listed
/// chapter with that title (it moved), else not. A same-titled chapter at a position the list
/// gives to a differently titled one is a different chapter that merely shares a name.
fn refers_to(list: &[ChapterRef], chapter: &ChapterRef) -> bool {
    if list.contains(chapter) {
        return true;
    }
    let mut same_title = list.iter().filter(|c| c.title == chapter.title);
    let first = same_title.next();
    first.is_some()
        && same_title.next().is_none()
        && !list
            .iter()
            .any(|c| c.index == chapter.index && c.title != chapter.title)
}

/// One point on the checklist.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChecklistItem {
    pub id: u64,
    pub text: String,
    /// Extra words that mean the same thing in the prose ("thief, theft, took").
    pub aliases: String,
    /// The chapters the writer has ticked this item off in.
    pub done_in: Vec<ChapterRef>,
    /// Where the point appears. A file from before scopes reads as the whole story.
    pub scope: Scope,
}

impl ChecklistItem {
    /// Whether the item is ticked in `chapter`.
    pub fn is_done_in(&self, chapter: &ChapterRef) -> bool {
        refers_to(&self.done_in, chapter)
    }

    /// Whether the item appears in `chapter`.
    pub fn applies_to(&self, chapter: &ChapterRef) -> bool {
        self.scope.includes(chapter)
    }

    fn set_done(&mut self, chapter: &ChapterRef, done: bool) {
        self.done_in
            .retain(|c| c != chapter && c.title != chapter.title);
        if done {
            self.done_in.push(chapter.clone());
        }
    }
}

/// Every item the writer has made, in the order they made them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Checklist {
    items: Vec<ChecklistItem>,
    next_id: u64,
}

impl Checklist {
    pub fn items(&self) -> &[ChecklistItem] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The items, for the characters module to lend names to a scored copy.
    pub(crate) fn items_mut(&mut self) -> &mut [ChecklistItem] {
        &mut self.items
    }

    pub fn get(&self, id: u64) -> Option<&ChecklistItem> {
        self.items.iter().find(|item| item.id == id)
    }

    /// Adds an item for the whole story and returns its id. Blank text adds nothing.
    pub fn add(&mut self, text: &str) -> Option<u64> {
        self.add_scoped(text, Scope::Story)
    }

    /// Adds an item that appears in `scope`. Blank text adds nothing.
    pub fn add_scoped(&mut self, text: &str, scope: Scope) -> Option<u64> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        self.next_id = self
            .next_id
            .max(self.items.iter().map(|i| i.id + 1).max().unwrap_or(0));
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(ChecklistItem {
            id,
            text: text.to_owned(),
            scope,
            ..ChecklistItem::default()
        });
        Some(id)
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|item| item.id != id);
    }

    pub fn set_text(&mut self, id: u64, text: String) {
        if let Some(item) = self.item_mut(id) {
            item.text = text;
        }
    }

    pub fn set_aliases(&mut self, id: u64, aliases: String) {
        if let Some(item) = self.item_mut(id) {
            item.aliases = aliases;
        }
    }

    pub fn set_done(&mut self, id: u64, chapter: &ChapterRef, done: bool) {
        if let Some(item) = self.item_mut(id) {
            item.set_done(chapter, done);
        }
    }

    fn item_mut(&mut self, id: u64) -> Option<&mut ChecklistItem> {
        self.items.iter_mut().find(|item| item.id == id)
    }
}

/// How the list is ordered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistOrder {
    /// The chapter uses the item's words least: probably still unaddressed.
    #[default]
    LeastCovered,
    /// The chapter uses the item's words most: probably what is being written now.
    MostRelevant,
}

/// How often each word stem occurs in one chapter.
///
/// It can be kept up to date as the chapter is edited: [`sync`](Self::sync) is given every
/// paragraph of the chapter and only looks at the words of those whose text changed, so a
/// keystroke costs one paragraph's worth of work however long the chapter is.
#[derive(Clone, Debug, Default)]
pub struct ChapterProfile {
    counts: HashMap<String, u32>,
    words: usize,
    /// The text each paragraph last contributed, by id, and the sync that last saw it.
    seen: HashMap<u64, (String, u32)>,
    pass: u32,
}

/// Two profiles are equal when they count the same words, however they were built.
impl PartialEq for ChapterProfile {
    fn eq(&self, other: &Self) -> bool {
        self.words == other.words && self.counts == other.counts
    }
}

impl Eq for ChapterProfile {}

impl ChapterProfile {
    /// Counts the stems of every paragraph's text.
    pub fn from_paragraphs<'a>(paragraphs: impl IntoIterator<Item = &'a str>) -> Self {
        let mut profile = Self::default();
        for text in paragraphs {
            profile.add(text);
        }
        profile
    }

    pub fn add(&mut self, text: &str) {
        for word in words(text) {
            self.words += 1;
            if let Some(stem) = keyword(word) {
                *self.counts.entry(stem).or_insert(0) += 1;
            }
        }
    }

    /// Takes back what [`add`](Self::add) counted for `text`.
    fn remove(&mut self, text: &str) {
        for word in words(text) {
            self.words = self.words.saturating_sub(1);
            if let Some(stem) = keyword(word)
                && let Some(count) = self.counts.get_mut(&stem)
            {
                *count -= 1;
                if *count == 0 {
                    self.counts.remove(&stem);
                }
            }
        }
    }

    /// Brings the profile in line with the chapter's paragraphs, each given as its id and text.
    /// Paragraphs whose text is the same as last time cost a comparison; changed ones are
    /// counted again; paragraphs that are no longer there are taken out. Returns whether the
    /// counts changed.
    pub fn sync<'a>(&mut self, paragraphs: impl IntoIterator<Item = (u64, &'a str)>) -> bool {
        self.pass = self.pass.wrapping_add(1);
        let pass = self.pass;
        let mut changed = false;
        for (id, text) in paragraphs {
            if let Some((old, seen)) = self.seen.get_mut(&id)
                && old == text
            {
                *seen = pass;
                continue;
            }
            changed = true;
            if let Some((old, _)) = self.seen.remove(&id) {
                self.remove(&old);
            }
            self.add(text);
            self.seen.insert(id, (text.to_owned(), pass));
        }
        let gone: Vec<u64> = self
            .seen
            .iter()
            .filter(|(_, (_, seen))| *seen != pass)
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            changed = true;
            if let Some((old, _)) = self.seen.remove(&id) {
                self.remove(&old);
            }
        }
        changed
    }

    /// Words in the chapter, stop words included.
    pub fn word_count(&self) -> usize {
        self.words
    }

    fn count(&self, stem: &str) -> u32 {
        self.counts.get(stem).copied().unwrap_or(0)
    }
}

/// How well a chapter covers one item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Score {
    /// 0 (none of its words used) to 1 (all used repeatedly). `None` when the item has no
    /// keywords to look for.
    pub coverage: Option<f32>,
    /// Times the chapter uses any of the item's keywords.
    pub mentions: u32,
}

/// Scores `item` against `profile`.
pub fn score(item: &ChecklistItem, profile: &ChapterProfile) -> Score {
    let own = keywords(&item.text);
    let mut aliases = keywords(&item.aliases);
    // A word the item already uses is not counted twice as an alias.
    aliases.retain(|stem| !own.contains(stem));
    let mut weight = 0.0_f32;
    let mut covered = 0.0_f32;
    let mut mentions = 0_u32;
    for (stems, w) in [(&own, 1.0_f32), (&aliases, ALIAS_WEIGHT)] {
        for stem in stems {
            let count = profile.count(stem);
            mentions += count;
            weight += w;
            // 1 - 0.5^count, so each further mention adds less.
            covered += w * (1.0 - 0.5_f32.powi(count.min(8) as i32));
        }
    }
    Score {
        coverage: (weight > 0.0).then(|| covered / weight),
        mentions,
    }
}

/// One row of the ranked list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ranked {
    pub id: u64,
    pub score: Score,
    pub done: bool,
}

/// The items that appear in `chapter`, in the order to show them: unticked items in `order`,
/// then ticked items. The order the writer made them in breaks ties, and items with no keywords
/// come after scored ones.
pub fn rank(
    checklist: &Checklist,
    profile: &ChapterProfile,
    chapter: &ChapterRef,
    order: ChecklistOrder,
) -> Vec<Ranked> {
    let mut rows: Vec<(usize, Ranked)> = checklist
        .items()
        .iter()
        .enumerate()
        .filter(|(_, item)| item.applies_to(chapter))
        .map(|(position, item)| {
            (
                position,
                Ranked {
                    id: item.id,
                    score: score(item, profile),
                    done: item.is_done_in(chapter),
                },
            )
        })
        .collect();
    rows.sort_by(|(pa, a), (pb, b)| {
        // Open items before done ones.
        a.done
            .cmp(&b.done)
            .then_with(|| match (a.score.coverage, b.score.coverage) {
                (Some(x), Some(y)) => {
                    let by_coverage = x.total_cmp(&y);
                    match order {
                        ChecklistOrder::LeastCovered => by_coverage,
                        ChecklistOrder::MostRelevant => by_coverage.reverse(),
                    }
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then(pa.cmp(pb))
    });
    rows.into_iter().map(|(_, ranked)| ranked).collect()
}

/// The words of `text`: runs of letters and digits, with an apostrophe kept inside a word.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '\u{2019}'))
        .map(|word| word.trim_matches(['\'', '\u{2019}']))
        .filter(|word| !word.is_empty())
}

/// The stem a word is counted under, or `None` for a stop word or a lone character.
fn keyword(word: &str) -> Option<String> {
    let lower = word.to_lowercase();
    // "mara's" is "mara"; "don't" stays whole and is a stop word's cousin, so it is dropped.
    let lower = lower
        .strip_suffix("'s")
        .or_else(|| lower.strip_suffix("\u{2019}s"))
        .unwrap_or(&lower);
    if lower.chars().count() < 2
        || STOP_WORDS
            .binary_search_by(|word| (*word).cmp(lower))
            .is_ok()
        || lower.contains('\'')
    {
        return None;
    }
    Some(stem(lower))
}

/// A rough stem: enough to match plural and tense forms, not a linguistic analysis.
/// Words with no ASCII letters (CJK) are left alone.
fn stem(word: &str) -> String {
    if !word.is_ascii() {
        return word.to_owned();
    }
    let mut w = word.to_owned();
    for suffix in ["ing", "ed", "es", "ly", "s"] {
        if w.len() >= suffix.len() + 4
            && let Some(rest) = w.strip_suffix(suffix)
        {
            w = rest.to_owned();
            break;
        }
    }
    // "stopped" and "stopping" lose a doubled final letter: stopp -> stop.
    let bytes = w.as_bytes();
    if w.len() >= 4
        && bytes[w.len() - 1] == bytes[w.len() - 2]
        && !b"lsz".contains(&bytes[w.len() - 1])
    {
        w.pop();
    }
    // A trailing silent "e" is dropped so "stable" and "stables" meet.
    if w.len() >= 5 && w.ends_with('e') {
        w.pop();
    }
    w
}

/// The distinct keyword stems of `text`, in first-seen order.
fn keywords(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for word in words(text) {
        if let Some(stem) = keyword(word)
            && !found.contains(&stem)
        {
            found.push(stem);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(text: &str) -> ChapterProfile {
        ChapterProfile::from_paragraphs([text])
    }

    fn item(text: &str) -> ChecklistItem {
        ChecklistItem {
            text: text.into(),
            ..ChecklistItem::default()
        }
    }

    #[test]
    fn stop_words_and_case_do_not_count() {
        assert_eq!(keywords("The Ledger and THE ledger"), ["ledger"]);
    }

    #[test]
    fn plural_and_tense_forms_meet() {
        let p = profile("She stopped. He was stopping them, and the ledgers stop here.");
        let s = score(&item("stop"), &p);
        assert_eq!(s.mentions, 3);
        assert!(s.coverage.unwrap() > 0.8);
    }

    #[test]
    fn an_absent_point_scores_zero_and_a_present_one_more() {
        let p = profile("Mara hid the ledger under the floorboards.");
        let missing = score(&item("Reveal the dragon"), &p);
        let present = score(&item("Mara hid the ledger"), &p);
        assert_eq!(missing.coverage, Some(0.0));
        assert!(present.coverage.unwrap() > 0.4);
    }

    #[test]
    fn repeating_a_word_adds_less_each_time() {
        let once = score(&item("ledger"), &profile("ledger")).coverage.unwrap();
        let twice = score(&item("ledger"), &profile("ledger ledger"))
            .coverage
            .unwrap();
        let many = score(&item("ledger"), &profile(&"ledger ".repeat(40)))
            .coverage
            .unwrap();
        assert!(once < twice && twice < many && many < 1.0001);
        assert!(many - twice < twice - once);
    }

    #[test]
    fn aliases_help_but_count_for_less() {
        let p = profile("The thief ran.");
        let plain = score(&item("Mara steals the ledger"), &p).coverage.unwrap();
        let mut with_alias = item("Mara steals the ledger");
        with_alias.aliases = "thief".into();
        let helped = score(&with_alias, &p).coverage.unwrap();
        assert_eq!(plain, 0.0);
        assert!(helped > 0.0);
        assert!(helped < 0.5);
    }

    #[test]
    fn an_item_with_no_keywords_has_no_coverage() {
        assert_eq!(
            score(&item("the and of"), &profile("anything")).coverage,
            None
        );
    }

    #[test]
    fn least_covered_first_by_default_and_most_relevant_on_request() {
        let mut list = Checklist::default();
        let ledger = list.add("ledger").unwrap();
        let dragon = list.add("dragon").unwrap();
        let p = profile("ledger ledger ledger and a little dragon");
        let chapter = ChapterRef::new(0, "One");
        let least = rank(&list, &p, &chapter, ChecklistOrder::LeastCovered);
        assert_eq!(
            least.iter().map(|r| r.id).collect::<Vec<_>>(),
            [dragon, ledger]
        );
        let most = rank(&list, &p, &chapter, ChecklistOrder::MostRelevant);
        assert_eq!(
            most.iter().map(|r| r.id).collect::<Vec<_>>(),
            [ledger, dragon]
        );
    }

    #[test]
    fn done_items_sink_below_open_ones() {
        let mut list = Checklist::default();
        let a = list.add("alpha").unwrap();
        let b = list.add("bravo").unwrap();
        let c = list.add("charlie").unwrap();
        let chapter = ChapterRef::new(0, "One");
        let p = profile("alpha alpha alpha charlie charlie charlie charlie");
        let ids = |list: &Checklist| -> Vec<u64> {
            rank(list, &p, &chapter, ChecklistOrder::LeastCovered)
                .iter()
                .map(|r| r.id)
                .collect()
        };
        assert_eq!(ids(&list), [b, a, c]);
        list.set_done(b, &chapter, true);
        assert_eq!(ids(&list), [a, c, b]);
        list.set_done(a, &chapter, true);
        assert_eq!(ids(&list), [c, b, a]);
    }

    #[test]
    fn ticks_belong_to_a_chapter_not_to_the_story() {
        let mut list = Checklist::default();
        let id = list.add("ledger").unwrap();
        let three = ChapterRef::new(2, "Three");
        let four = ChapterRef::new(3, "Four");
        list.set_done(id, &three, true);
        let item = list.get(id).unwrap();
        assert!(item.is_done_in(&three));
        assert!(!item.is_done_in(&four));
        // Chapter "Three" moved to position 4 (one inserted before it): the tick follows.
        assert!(item.is_done_in(&ChapterRef::new(3, "Three")));
        list.set_done(id, &three, false);
        assert!(!list.get(id).unwrap().is_done_in(&three));
    }

    #[test]
    fn a_point_appears_only_in_the_chapters_its_scope_names() {
        let (one, two, three, four) = (
            ChapterRef::new(0, "One"),
            ChapterRef::new(1, "Two"),
            ChapterRef::new(2, "Three"),
            ChapterRef::new(3, "Four"),
        );
        assert!(
            [&one, &two, &three, &four]
                .iter()
                .all(|c| Scope::Story.includes(c))
        );

        let only_two = Scope::only(two.clone());
        assert!(only_two.includes(&two));
        assert!(
            !only_two.includes(&one) && !only_two.includes(&three) && !only_two.includes(&four)
        );

        // Two and four picked, nothing after.
        let picked = Scope::Chapters {
            chapters: vec![two.clone(), four.clone()],
            onward: false,
        };
        assert!(picked.includes(&two) && picked.includes(&four));
        assert!(!picked.includes(&one) && !picked.includes(&three));
        assert!(!picked.includes(&ChapterRef::new(4, "Five")));

        // Two picked, and everything after it.
        let onward = Scope::Chapters {
            chapters: vec![two.clone()],
            onward: true,
        };
        assert!(!onward.includes(&one));
        assert!(onward.includes(&two) && onward.includes(&three) && onward.includes(&four));
    }

    #[test]
    fn onward_starts_after_the_last_picked_chapter() {
        let scope = Scope::Chapters {
            chapters: vec![ChapterRef::new(1, "Two"), ChapterRef::new(4, "Five")],
            onward: true,
        };
        assert!(scope.includes(&ChapterRef::new(1, "Two")));
        assert!(!scope.includes(&ChapterRef::new(2, "Three")));
        assert!(!scope.includes(&ChapterRef::new(3, "Four")));
        assert!(scope.includes(&ChapterRef::new(4, "Five")));
        assert!(scope.includes(&ChapterRef::new(5, "Six")));
    }

    #[test]
    fn ranking_leaves_out_points_that_are_not_for_this_chapter() {
        let mut list = Checklist::default();
        let one = ChapterRef::new(0, "One");
        let two = ChapterRef::new(1, "Two");
        let everywhere = list.add("ledger").unwrap();
        let here = list.add_scoped("dragon", Scope::only(one.clone())).unwrap();
        let there = list
            .add_scoped("harbour", Scope::only(two.clone()))
            .unwrap();
        let p = profile("anything");
        let ids = |chapter: &ChapterRef| -> Vec<u64> {
            rank(&list, &p, chapter, ChecklistOrder::LeastCovered)
                .iter()
                .map(|r| r.id)
                .collect()
        };
        assert_eq!(ids(&one), [everywhere, here]);
        assert_eq!(ids(&two), [everywhere, there]);
    }

    #[test]
    fn the_stop_word_list_is_sorted_for_binary_search() {
        assert!(STOP_WORDS.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn syncing_after_edits_counts_the_same_as_starting_again() {
        let mut live = ChapterProfile::default();
        let first = [
            (1, "Mara hid the ledger."),
            (2, "The dragon circled the harbour twice."),
            (3, "Tobias laughed."),
        ];
        assert!(live.sync(first));
        assert_eq!(
            live,
            ChapterProfile::from_paragraphs(first.iter().map(|(_, t)| *t))
        );
        // Nothing changed: no work, and it says so.
        assert!(!live.sync(first));

        // One paragraph edited, one deleted, one added.
        let second = [
            (1, "Mara hid the ledger and the key."),
            (3, "Tobias laughed."),
            (4, "The harbour master frowned."),
        ];
        assert!(live.sync(second));
        assert_eq!(
            live,
            ChapterProfile::from_paragraphs(second.iter().map(|(_, t)| *t))
        );
        assert_eq!(
            score(&item("dragon"), &live).mentions,
            0,
            "a deleted paragraph is gone"
        );
        assert_eq!(score(&item("key"), &live).mentions, 1);

        // Emptied completely.
        assert!(live.sync([]));
        assert_eq!(live, ChapterProfile::default());
    }

    /// Timing for a long chapter: `cargo test --release -p libertum-model -- --ignored --nocapture
    /// scoring_cost`.
    #[test]
    #[ignore = "a timing probe, not a check"]
    fn scoring_cost_on_a_long_chapter() {
        use std::time::Instant;
        let sentence = "Mara hid the ledger under the floorboards while the dragon circled the harbour twice and Tobias laughed at nothing in particular. ";
        let paragraphs: Vec<String> = (0..1500)
            .map(|i| format!("{sentence}{sentence}Paragraph number {i} ends here."))
            .collect();
        let words: usize = paragraphs
            .iter()
            .map(|p| p.split_whitespace().count())
            .sum();
        let ids: Vec<(u64, &str)> = paragraphs
            .iter()
            .enumerate()
            .map(|(i, p)| (i as u64, p.as_str()))
            .collect();

        let started = Instant::now();
        let full = ChapterProfile::from_paragraphs(paragraphs.iter().map(String::as_str));
        let rebuild = started.elapsed();

        let mut live = ChapterProfile::default();
        let started = Instant::now();
        live.sync(ids.iter().copied());
        let first_sync = started.elapsed();
        assert_eq!(live, full);

        let started = Instant::now();
        let changed = live.sync(ids.iter().copied());
        let unchanged = started.elapsed();
        assert!(!changed);

        let mut edited = paragraphs.clone();
        edited[700].push_str(" and a dragon");
        let ids2: Vec<(u64, &str)> = edited
            .iter()
            .enumerate()
            .map(|(i, p)| (i as u64, p.as_str()))
            .collect();
        let started = Instant::now();
        live.sync(ids2.iter().copied());
        let one_edit = started.elapsed();

        let mut list = Checklist::default();
        for text in [
            "Mara hides the ledger",
            "dragon attacks harbour",
            "Tobias betrays Mara",
            "old debt fire",
            "storm at sea",
        ] {
            list.add(text);
        }
        let chapter = ChapterRef::new(0, "One");
        let started = Instant::now();
        let _ = rank(&list, &live, &chapter, ChecklistOrder::LeastCovered);
        let ranking = started.elapsed();
        println!(
            "{words} words, {} paragraphs\n  full recount            {rebuild:?}\n  first sync              {first_sync:?}\n  sync, nothing changed   {unchanged:?}\n  sync, one paragraph     {one_edit:?}\n  rank 5 points           {ranking:?}",
            paragraphs.len()
        );
    }

    #[test]
    fn a_scope_survives_a_round_trip_and_an_old_item_reads_as_the_whole_story() {
        let mut list = Checklist::default();
        list.add_scoped(
            "dragon",
            Scope::Chapters {
                chapters: vec![ChapterRef::new(1, "Two")],
                onward: true,
            },
        )
        .unwrap();
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(serde_json::from_str::<Checklist>(&json).unwrap(), list);
        let old = r#"{"items":[{"id":0,"text":"ledger","aliases":"","pinned":false,"done_in":[]}],"next_id":1}"#;
        let read: Checklist = serde_json::from_str(old).unwrap();
        assert_eq!(read.items()[0].scope, Scope::Story);
    }

    #[test]
    fn blank_items_are_refused_and_ids_are_not_reused() {
        let mut list = Checklist::default();
        assert_eq!(list.add("   "), None);
        let a = list.add("one").unwrap();
        list.remove(a);
        let b = list.add("two").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn survives_a_round_trip_and_reads_an_empty_object() {
        let mut list = Checklist::default();
        let id = list.add("ledger").unwrap();
        list.set_aliases(id, "book".into());
        list.set_done(id, &ChapterRef::new(0, "One"), true);
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(serde_json::from_str::<Checklist>(&json).unwrap(), list);
        assert!(serde_json::from_str::<Checklist>("{}").unwrap().is_empty());
    }
}
