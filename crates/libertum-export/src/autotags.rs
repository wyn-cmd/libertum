//! Tag suggestions drawn from a chapter's own text.
//!
//! [`crate::tags`] ranks a vocabulary the writer keeps. This module needs no vocabulary: it
//! reads the chapter and proposes characters, relationships and additional tags, so a new
//! story gets suggestions before a single tag is typed. Everything is a plain-text heuristic;
//! nothing leaves the computer and nothing is applied. Each suggestion carries its evidence
//! and the review list starts unticked.
//!
//! Limits:
//!
//! * **Characters** are capitalised words that recur and are not just the first word of a
//!   sentence. A name that is also an ordinary word (`Hope`, `Mark`) is missed when the
//!   chapter uses it in lower case too. A name seen only at the start of sentences needs more
//!   mentions, and an introductory word ("Suddenly,") is told from a name by its ending, a
//!   comma after it, or a short list. A title in front of a name (`Captain Voss`) is dropped
//!   when the name mostly appears without it.
//! * **Relationships** are pairs of those characters who keep turning up in the same
//!   paragraph. Text alone cannot tell a romance from a friendship, so a pair is written the
//!   romantic way (`A/B`) only when romantic cue words (kiss, love, lips ...) show up in
//!   paragraphs they share; otherwise it is written the platonic way (`A & B`).
//! * **Full names.** A [`NameBook`] holds the full names (two or three capitalised words, seen
//!   at least twice) of the whole story, plus any the writer has used before. A chapter that
//!   says only "Mara" suggests "Mara Ellison" when that is the one full name with "Mara" in
//!   it; a part shared by two full names ("Ellison" in "Mara Ellison" and "James Ellison")
//!   stays as it is. Mentions are counted over every form of the name, and pairings use the
//!   full names and count a paragraph that uses any form.
//! * **Additional tags** are the words and two-word phrases the chapter leans on most, after
//!   names, function words and a list of very common narrative words are set aside. There is
//!   no corpus to compare against, so this list is the noisiest of the three.
//!
//! The fandom cannot be told from a chapter's text and is never suggested, and neither are
//! ratings or warnings, which the writer chooses on AO3.

use std::collections::{HashMap, HashSet};

use crate::tags::{Tag, TagCategory, TagMatch, TagSuggestion, is_stopword, stem};

/// Times a run of capitalised words must appear in the whole story to count as a full name.
const MIN_FULL_NAME: usize = 2;
/// Times a two-word phrase must appear in a chapter to be suggested.
const MIN_PHRASE: usize = 3;
/// Most two-word phrases among the keyword suggestions.
const MAX_PHRASES: usize = 3;

/// The full names of a story's characters, so a chapter's "Mara" can be suggested as
/// "Mara Ellison". Built from the whole story's text, and topped up with names the writer used
/// before (a remembered name never overrides one the story itself uses).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameBook {
    /// Each full name as its words.
    full: Vec<Vec<String>>,
}

impl NameBook {
    /// The full names in `text`: runs of two or three capitalised words seen at least twice.
    pub fn from_text(text: &str) -> Self {
        let paragraphs: Vec<&str> = text
            .split('\n')
            .map(str::trim)
            .filter(|paragraph| !paragraph.is_empty())
            .collect();
        let scan = scan_names(&paragraphs);
        let mut totals: HashMap<&String, usize> = HashMap::new();
        for (run, count) in scan.runs.iter().chain(&scan.tails) {
            *totals.entry(run).or_insert(0) += count;
        }
        let mut runs: Vec<(&String, usize)> = totals
            .into_iter()
            .filter(|(run, count)| {
                *count >= MIN_FULL_NAME
                    && run
                        .split(' ')
                        .all(|part| !scan.lower.contains(&part.to_lowercase()))
            })
            .collect();
        // "Later Mara Ellison" gives way to "Mara Ellison" when the shorter name is seen as
        // often: the extra word is a sentence's opening word.
        let counts: HashMap<&String, usize> = runs.iter().map(|(r, c)| (*r, *c)).collect();
        runs.retain(|(run, count)| {
            run.split_once(' ').is_none_or(|(_, tail)| {
                !tail.contains(' ')
                    || !counts
                        .iter()
                        .any(|(other, c)| other.as_str() == tail && *c >= *count)
            })
        });
        // Most used first, so a longer, rarer run does not shadow the usual full name.
        runs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let mut book = Self::default();
        for (run, _) in runs {
            let parts: Vec<String> = run.split(' ').map(str::to_owned).collect();
            // A run inside a full name already kept ("Mara Jane Ellison" vs "Jane Ellison")
            // is the same person.
            let inside = book
                .full
                .iter()
                .any(|full| parts.iter().all(|part| full.contains(part)));
            if !inside {
                book.full.push(parts);
            }
        }
        book
    }

    /// Adds full names remembered from earlier use. One whose words the story already uses
    /// in a full name of its own is left out: the story's own names win.
    pub fn remember<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        for name in names {
            let parts: Vec<String> = name.split_whitespace().map(str::to_owned).collect();
            if parts.len() < 2 || parts.len() > MAX_NAME_WORDS {
                continue;
            }
            let clashes = self
                .full
                .iter()
                .any(|full| parts.iter().any(|part| full.contains(part)));
            if !clashes {
                self.full.push(parts);
            }
        }
    }

    /// The full names, as text.
    pub fn names(&self) -> Vec<String> {
        self.full.iter().map(|parts| parts.join(" ")).collect()
    }

    fn contains(&self, name: &str) -> bool {
        self.full.iter().any(|parts| parts.join(" ") == name)
    }

    /// The one full name `part` belongs to, if exactly one has it.
    fn resolve(&self, part: &str) -> Option<&Vec<String>> {
        let mut found = self
            .full
            .iter()
            .filter(|full| full.iter().any(|p| p == part));
        match (found.next(), found.next()) {
            (Some(only), None) => Some(only),
            _ => None,
        }
    }

    /// The single words that stand for `full` alone (its parts no other full name has).
    fn aliases(&self, full: &[String]) -> Vec<String> {
        full.iter()
            .filter(|part| self.resolve(part).is_some_and(|f| f.as_slice() == full))
            .cloned()
            .collect()
    }
}

/// A character found in a chapter: the name to suggest and every form the text uses.
struct Character {
    name: String,
    mentions: usize,
    /// The full name (if it has more than one word) and the single words that stand for it.
    forms: Vec<String>,
    /// The short forms this chapter actually uses, for the evidence line.
    used_short: Vec<String>,
}

/// Most suggestions of each kind.
pub const MAX_CHARACTERS: usize = 8;
pub const MAX_RELATIONSHIPS: usize = 4;
pub const MAX_KEYWORDS: usize = 8;

/// A name must appear at least this often to count as a character.
const MIN_MENTIONS: usize = 3;
/// A name seen only at the start of sentences needs this many mentions: with no mid-sentence
/// use to go on, it has to recur more often before it is believed.
const MIN_INITIAL_ONLY: usize = 4;
/// Two characters must share at least this many paragraphs to count as a pair.
const MIN_TOGETHER: usize = 3;
/// A pair is written romantically when this many of its shared paragraphs have a cue word.
const MIN_ROMANTIC_PARAGRAPHS: usize = 2;
/// A word must appear at least this often to be offered as an additional tag.
const MIN_KEYWORD: usize = 4;
/// Only this many of the most mentioned characters are considered for pairs.
const PAIR_POOL: usize = 6;
/// Longest run of capitalised words taken as one name (`Captain Mara Voss`).
const MAX_NAME_WORDS: usize = 3;

/// Capitalised words that are not names.
const NOT_NAMES: &[&str] = &[
    "i",
    "mr",
    "mrs",
    "ms",
    "dr",
    "sir",
    "lord",
    "lady",
    "miss",
    "madam",
    "god",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "christmas",
    "english",
    "yes",
    "no",
    "okay",
    "oh",
    "ah",
    "well",
    "hey",
    "hello",
    "please",
    "thanks",
    "thank",
    "chapter",
];

/// Abbreviations whose full stop does not end a sentence.
const ABBREVIATIONS: &[&str] = &["mr", "mrs", "ms", "dr", "st", "mt", "sr", "jr"];

/// Very common words in fiction that say nothing about a story. Together with the function
/// words of [`is_stopword`] they are never offered as additional tags.
const COMMON_WORDS: &[&str] = &[
    "said",
    "says",
    "asked",
    "replied",
    "answered",
    "looked",
    "looking",
    "looks",
    "went",
    "going",
    "gone",
    "came",
    "coming",
    "comes",
    "like",
    "liked",
    "back",
    "know",
    "knew",
    "known",
    "think",
    "thought",
    "thinks",
    "would",
    "could",
    "should",
    "might",
    "must",
    "just",
    "even",
    "still",
    "also",
    "then",
    "there",
    "here",
    "away",
    "into",
    "over",
    "out",
    "down",
    "off",
    "after",
    "before",
    "again",
    "around",
    "through",
    "between",
    "something",
    "nothing",
    "anything",
    "everything",
    "someone",
    "anyone",
    "everyone",
    "every",
    "much",
    "many",
    "more",
    "most",
    "some",
    "other",
    "another",
    "such",
    "only",
    "very",
    "really",
    "never",
    "always",
    "ever",
    "maybe",
    "perhaps",
    "however",
    "though",
    "although",
    "because",
    "while",
    "until",
    "since",
    "without",
    "within",
    "upon",
    "toward",
    "towards",
    "been",
    "being",
    "made",
    "make",
    "makes",
    "take",
    "took",
    "taken",
    "taking",
    "see",
    "saw",
    "seen",
    "want",
    "wanted",
    "need",
    "needed",
    "tell",
    "told",
    "turn",
    "turned",
    "give",
    "gave",
    "given",
    "find",
    "found",
    "feel",
    "felt",
    "seem",
    "seemed",
    "seems",
    "keep",
    "kept",
    "left",
    "right",
    "long",
    "little",
    "good",
    "great",
    "small",
    "first",
    "last",
    "own",
    "same",
    "back",
    "come",
    "came",
    "get",
    "gets",
    "got",
    "getting",
    "let",
    "lets",
    "say",
    "put",
    "set",
    "hand",
    "hands",
    "eyes",
    "face",
    "head",
    "time",
    "moment",
    "something",
    "word",
    "words",
    "thing",
    "things",
    "people",
    "man",
    "woman",
    "day",
    "night",
    "way",
    "door",
    "room",
    "voice",
    "once",
    "though",
    "toward",
    "yet",
    "both",
    "each",
    "well",
    "better",
    "enough",
    "least",
    "less",
    "next",
    "whole",
    "half",
    "soon",
    "already",
    "almost",
    "quite",
    "rather",
    "else",
    "instead",
    "whether",
    "whose",
    "whom",
    "these",
    "those",
    "than",
    "that",
    "this",
    "what",
    "which",
    "where",
    "when",
    "with",
    "from",
    "have",
    "has",
    "had",
    "does",
    "did",
    "done",
    "doing",
    "were",
    "was",
    "are",
    "the",
    "and",
];

/// Words that suggest a romantic scene, compared by stem.
const ROMANTIC_CUES: &[&str] = &[
    "kiss",
    "love",
    "lover",
    "lips",
    "embrace",
    "beloved",
    "darling",
    "sweetheart",
    "romance",
    "desire",
    "cuddle",
    "caress",
    "passion",
    "adore",
];

/// Suggestions drawn from `text` alone: characters first, then relationships, then additional
/// tags. Deterministic for a given text.
pub fn auto_suggestions(text: &str) -> Vec<TagSuggestion> {
    auto_suggestions_with(text, &NameBook::from_text(text))
}

/// [`auto_suggestions`] with the story's full names (see [`NameBook`]).
pub fn auto_suggestions_with(text: &str, book: &NameBook) -> Vec<TagSuggestion> {
    let paragraphs: Vec<&str> = text
        .split('\n')
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .collect();
    let scan = scan_names(&paragraphs);
    let characters = resolve_characters(&paragraphs, characters(&scan), book);
    let relationships = relationships(&paragraphs, &characters);
    let name_words: HashSet<String> = characters
        .iter()
        .flat_map(|c| {
            c.forms
                .iter()
                .flat_map(|f| f.split(' ').map(str::to_lowercase))
        })
        .chain(scan.all_name_words())
        .collect();
    let keywords = keywords(&paragraphs, &name_words);

    let mut out = Vec::new();
    for character in &characters {
        // "mentions (as Mara, Ellison)" when the chapter also uses the short forms.
        let term = if character.name.contains(' ') && !character.used_short.is_empty() {
            format!("mentions (as {})", character.used_short.join(", "))
        } else {
            "mentions".to_owned()
        };
        out.push(suggestion(
            character.name.clone(),
            TagCategory::Character,
            character.mentions as f32,
            vec![TagMatch {
                term,
                count: character.mentions,
            }],
        ));
    }
    out.extend(relationships);
    for (word, count) in keywords {
        out.push(suggestion(
            word.split(' ')
                .map(capitalise)
                .collect::<Vec<_>>()
                .join(" "),
            TagCategory::Freeform,
            count as f32,
            vec![TagMatch { term: word, count }],
        ));
    }
    out
}

/// `vocabulary` suggestions followed by the `auto` ones the vocabulary does not already cover
/// (compared by name, ignoring case), so a tag the writer keeps is never offered twice.
pub fn merge_suggestions(
    vocabulary: Vec<TagSuggestion>,
    auto: Vec<TagSuggestion>,
) -> Vec<TagSuggestion> {
    let known: HashSet<String> = vocabulary
        .iter()
        .map(|suggestion| suggestion.tag.name.to_lowercase())
        .collect();
    let mut merged = vocabulary;
    merged.extend(
        auto.into_iter()
            .filter(|suggestion| !known.contains(&suggestion.tag.name.to_lowercase())),
    );
    merged
}

fn suggestion(
    name: String,
    category: TagCategory,
    score: f32,
    matched: Vec<TagMatch>,
) -> TagSuggestion {
    TagSuggestion {
        tag: Tag::new(name, category),
        score,
        matched,
    }
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ------------------------------------------------------------------ reading words

/// One word of a paragraph and whether it opens a sentence (or a quotation).
struct Word {
    text: String,
    initial: bool,
    /// The word is followed straight away by a comma ("Suddenly, ..."), which marks an
    /// introductory word rather than a name.
    comma: bool,
}

/// The words of one paragraph. An apostrophe or a hyphen inside a word stays in it, a
/// possessive `'s` is taken off, and a full stop after an abbreviation (`Mr.`) does not end
/// the sentence. A quotation opens a new sentence; its closing mark does not.
fn words(paragraph: &str) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::new();
    let mut buffer = String::new();
    let mut initial = true;
    let mut in_quote = false;
    let chars: Vec<char> = paragraph.chars().collect();

    let flush = |buffer: &mut String, out: &mut Vec<Word>, initial: &mut bool, comma: bool| {
        if buffer.is_empty() {
            return;
        }
        let mut word = std::mem::take(buffer);
        for suffix in ["'s", "\u{2019}s"] {
            if let Some(stripped) = word.strip_suffix(suffix) {
                word = stripped.to_owned();
            }
        }
        let word = word
            .trim_matches(|c: char| c == '\'' || c == '\u{2019}' || c == '-')
            .to_owned();
        if !word.is_empty() {
            out.push(Word {
                text: word,
                initial: *initial,
                comma,
            });
            *initial = false;
        }
    };

    for (index, &c) in chars.iter().enumerate() {
        let next_is_letter = chars.get(index + 1).is_some_and(|n| n.is_alphabetic());
        if c.is_alphabetic()
            || (matches!(c, '\'' | '\u{2019}' | '-') && !buffer.is_empty() && next_is_letter)
        {
            buffer.push(c);
            continue;
        }
        flush(&mut buffer, &mut out, &mut initial, c == ',');
        match c {
            '.' | '!' | '?' | '\u{2026}' => {
                let abbreviation = c == '.'
                    && out
                        .last()
                        .is_some_and(|w| ABBREVIATIONS.contains(&w.text.to_lowercase().as_str()));
                if !abbreviation {
                    initial = true;
                }
            }
            '"' => {
                in_quote = !in_quote;
                if in_quote {
                    initial = true;
                }
            }
            '\u{201c}' | '\u{2018}' => {
                in_quote = true;
                initial = true;
            }
            '\u{201d}' => in_quote = false,
            _ => {}
        }
    }
    flush(&mut buffer, &mut out, &mut initial, false);
    out
}

fn is_capitalised(word: &str) -> bool {
    let mut letters = word.chars().filter(|c| c.is_alphabetic());
    let Some(first) = letters.next() else {
        return false;
    };
    let rest: Vec<char> = letters.collect();
    first.is_uppercase() && !rest.is_empty() && !rest.iter().all(|c| c.is_uppercase())
}

// ------------------------------------------------------------------ characters

/// What a pass over the text found about capitalised words.
struct NameScan {
    /// Occurrences of each capitalised word away from the start of a sentence.
    middle: HashMap<String, usize>,
    /// Occurrences of each capitalised word at the start of a sentence.
    initial: HashMap<String, usize>,
    /// How many of a word's sentence-initial occurrences are followed by a comma.
    initial_comma: HashMap<String, usize>,
    /// Occurrences of each run of two or more capitalised words.
    runs: HashMap<String, usize>,
    /// Runs of three that open a sentence, without their first word ("Later Mara Ellison"
    /// gives "Mara Ellison"): the opening word may be an ordinary word, capitalised.
    tails: HashMap<String, usize>,
    /// Every word that also appears in lower case.
    lower: HashSet<String>,
}

impl NameScan {
    /// The single-word names that qualified, lowercased, so keywords leave them out.
    fn all_name_words(&self) -> impl Iterator<Item = String> + '_ {
        self.middle.keys().map(|word| word.to_lowercase())
    }
}

fn scan_names(paragraphs: &[&str]) -> NameScan {
    let mut scan = NameScan {
        middle: HashMap::new(),
        initial: HashMap::new(),
        initial_comma: HashMap::new(),
        runs: HashMap::new(),
        tails: HashMap::new(),
        lower: HashSet::new(),
    };
    for paragraph in paragraphs {
        let words = words(paragraph);
        let mut run: Vec<&str> = Vec::new();
        // The run's first word opened a sentence.
        let mut opens = false;
        let flush_run = |run: &mut Vec<&str>, scan: &mut NameScan, opens: bool| {
            if run.len() >= 2 {
                *scan.runs.entry(run.join(" ")).or_insert(0) += 1;
            }
            if run.len() >= 3 && opens {
                *scan.tails.entry(run[1..].join(" ")).or_insert(0) += 1;
            }
            run.clear();
        };
        for word in &words {
            let lower = word.text.to_lowercase();
            let name_like = is_capitalised(&word.text)
                && !NOT_NAMES.contains(&lower.as_str())
                && !is_stopword(&lower);
            if !name_like {
                if word.text.chars().next().is_some_and(char::is_lowercase) {
                    scan.lower.insert(lower);
                }
                flush_run(&mut run, &mut scan, opens);
                continue;
            }
            // A new sentence starts a new run: "...Voss. Mara..." is two names, not one.
            if word.initial {
                flush_run(&mut run, &mut scan, opens);
                *scan.initial.entry(word.text.clone()).or_insert(0) += 1;
                if word.comma {
                    *scan.initial_comma.entry(word.text.clone()).or_insert(0) += 1;
                }
            } else {
                *scan.middle.entry(word.text.clone()).or_insert(0) += 1;
            }
            if run.len() == MAX_NAME_WORDS {
                flush_run(&mut run, &mut scan, opens);
            }
            if run.is_empty() {
                opens = word.initial;
            }
            run.push(&word.text);
            // A comma ends a name: "Later, Mara Ellison" is not "Later Mara Ellison".
            if word.comma {
                flush_run(&mut run, &mut scan, opens);
            }
        }
        flush_run(&mut run, &mut scan, opens);
    }
    scan
}

/// The characters, most mentioned first, as `(name, mentions)`.
fn characters(scan: &NameScan) -> Vec<(String, usize)> {
    let mut found: Vec<(String, usize)> = Vec::new();

    // A name that is also a word the chapter uses in lower case is more likely the word.
    // A name used only at the start of sentences could be any word that was capitalised.
    let single = |name: &str| -> usize {
        let middle = scan.middle.get(name).copied().unwrap_or(0);
        let initial = scan.initial.get(name).copied().unwrap_or(0);
        let lower = name.to_lowercase();
        if scan.lower.contains(&lower) {
            return 0;
        }
        if middle > 0 {
            return middle + initial;
        }
        // Only ever seen opening a sentence. Names do that all the time ("Voss stood
        // watch."), and so do introductory words ("Suddenly, ..."), which are told apart by
        // an -ly ending, a comma straight after, or the NOT_NAMES list (already screened).
        let commas = scan.initial_comma.get(name).copied().unwrap_or(0);
        if initial >= MIN_INITIAL_ONLY && !lower.ends_with("ly") && commas * 10 <= initial * 4 {
            initial
        } else {
            0
        }
    };

    let mut run_counts: Vec<(&String, usize)> = scan
        .runs
        .iter()
        .map(|(run, count)| (run, *count))
        .filter(|(_, count)| *count >= MIN_MENTIONS)
        .collect();
    run_counts.sort();

    for name in scan.middle.keys().chain(scan.initial.keys()) {
        if found.iter().any(|(existing, _)| existing == name) {
            continue;
        }
        let total = single(name);
        if total < MIN_MENTIONS {
            continue;
        }
        // Mostly seen inside a longer name that qualifies: that longer name is the tag.
        let inside: usize = run_counts
            .iter()
            .filter(|(run, _)| run.split(' ').any(|part| part == name))
            .map(|(_, count)| *count)
            .sum();
        if inside * 10 >= total * 9 {
            continue;
        }
        found.push((name.clone(), total));
    }
    for (run, count) in run_counts {
        found.push((run.clone(), count));
    }
    found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    found
}

/// Folds the names found in a chapter into the story's full names, counts every form of each
/// name, and keeps the most mentioned.
fn resolve_characters(
    paragraphs: &[&str],
    found: Vec<(String, usize)>,
    book: &NameBook,
) -> Vec<Character> {
    let mut out: Vec<Character> = Vec::new();
    for (name, _) in found {
        let full: Option<Vec<String>> = if book.contains(&name) {
            Some(name.split(' ').map(str::to_owned).collect())
        } else if !name.contains(' ') {
            book.resolve(&name).cloned()
        } else {
            // "Later Mara Ellison": a sentence's opening word in front of a known full name.
            name.split_once(' ')
                .filter(|(_, tail)| book.contains(tail))
                .map(|(_, tail)| tail.split(' ').map(str::to_owned).collect())
        };
        let (canonical, forms) = match full {
            Some(parts) => {
                let joined = parts.join(" ");
                let mut forms = vec![joined.clone()];
                forms.extend(book.aliases(&parts));
                (joined, forms)
            }
            None => (name.clone(), vec![name]),
        };
        if out.iter().any(|c| c.name == canonical) {
            continue;
        }
        let (mentions, used_short) = count_mentions(paragraphs, &forms);
        if mentions >= MIN_MENTIONS {
            out.push(Character {
                name: canonical,
                mentions,
                forms,
                used_short,
            });
        }
    }
    out.sort_by(|a, b| {
        b.mentions
            .cmp(&a.mentions)
            .then_with(|| a.name.cmp(&b.name))
    });
    out.truncate(MAX_CHARACTERS);
    out
}

/// How often any of `forms` appears (a multi-word form counts once, not once per word), and
/// which single-word forms were used, in the order they first appear.
fn count_mentions(paragraphs: &[&str], forms: &[String]) -> (usize, Vec<String>) {
    let long: Vec<Vec<&str>> = forms
        .iter()
        .filter(|f| f.contains(' '))
        .map(|f| f.split(' ').collect())
        .collect();
    let single: HashSet<&str> = forms
        .iter()
        .filter(|f| !f.contains(' '))
        .map(String::as_str)
        .collect();
    let mut count = 0;
    let mut used: Vec<String> = Vec::new();
    for paragraph in paragraphs {
        let words = words(paragraph);
        let mut i = 0;
        while i < words.len() {
            if let Some(n) = long.iter().map(Vec::len).find(|&n| {
                long.iter().any(|parts| {
                    parts.len() == n
                        && words.len() >= i + n
                        && parts
                            .iter()
                            .zip(&words[i..i + n])
                            .all(|(p, w)| *p == w.text)
                })
            }) {
                count += 1;
                i += n;
                continue;
            }
            if single.contains(words[i].text.as_str()) {
                count += 1;
                if !used.contains(&words[i].text) {
                    used.push(words[i].text.clone());
                }
            }
            i += 1;
        }
    }
    (count, used)
}

// ------------------------------------------------------------------ relationships

/// True when `name` appears in `paragraph` as a whole word.
fn mentions(paragraph: &str, name: &str) -> bool {
    paragraph.match_indices(name).any(|(start, _)| {
        let before = paragraph[..start].chars().next_back();
        let after = paragraph[start + name.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

fn relationships(paragraphs: &[&str], characters: &[Character]) -> Vec<TagSuggestion> {
    let pool: Vec<&Character> = characters.iter().take(PAIR_POOL).collect();
    if pool.len() < 2 {
        return Vec::new();
    }
    let cues: HashSet<String> = ROMANTIC_CUES.iter().map(|cue| stem(cue)).collect();
    // (together, with a romantic cue word) per pair of pool indices, first before second.
    let mut pairs: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
    for paragraph in paragraphs {
        let present: Vec<usize> = pool
            .iter()
            .enumerate()
            .filter(|(_, character)| character.forms.iter().any(|form| mentions(paragraph, form)))
            .map(|(index, _)| index)
            .collect();
        if present.len() < 2 {
            continue;
        }
        let cue = paragraph
            .split(|c: char| !c.is_alphabetic())
            .filter(|word| !word.is_empty())
            .any(|word| cues.contains(&stem(&word.to_lowercase())));
        for (position, &a) in present.iter().enumerate() {
            for &b in &present[position + 1..] {
                let entry = pairs.entry((a, b)).or_insert((0, 0));
                entry.0 += 1;
                if cue {
                    entry.1 += 1;
                }
            }
        }
    }
    let mut ranked: Vec<((usize, usize), (usize, usize))> = pairs
        .into_iter()
        .filter(|(_, (together, _))| *together >= MIN_TOGETHER)
        .collect();
    ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(MAX_RELATIONSHIPS);
    ranked
        .into_iter()
        .map(|((a, b), (together, cue))| {
            let romantic = cue >= MIN_ROMANTIC_PARAGRAPHS;
            let separator = if romantic { "/" } else { " & " };
            let name = format!("{}{separator}{}", pool[a].name, pool[b].name);
            let mut matched = vec![TagMatch {
                term: "paragraphs together".to_owned(),
                count: together,
            }];
            if romantic {
                matched.push(TagMatch {
                    term: "romantic cue words in".to_owned(),
                    count: cue,
                });
            }
            suggestion(name, TagCategory::Relationship, together as f32, matched)
        })
        .collect()
}

// ------------------------------------------------------------------ keywords

/// The words the chapter leans on, as `(word, count)`, most used first.
fn keywords(paragraphs: &[&str], names: &HashSet<String>) -> Vec<(String, usize)> {
    // stem -> (total, surface form -> count)
    let mut groups: HashMap<String, (usize, HashMap<String, usize>)> = HashMap::new();
    for paragraph in paragraphs {
        for word in words(paragraph) {
            let lower = word.text.to_lowercase();
            if lower.chars().count() < 4
                || names.contains(&lower)
                || is_stopword(&lower)
                || COMMON_WORDS.contains(&lower.as_str())
                || !lower.chars().all(char::is_alphabetic)
            {
                continue;
            }
            let entry = groups.entry(stem(&lower)).or_default();
            entry.0 += 1;
            *entry.1.entry(lower).or_insert(0) += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = groups
        .into_values()
        .filter(|(total, _)| *total >= MIN_KEYWORD)
        .map(|(total, forms)| {
            // Show the form the chapter uses most (the shorter one on a tie).
            let mut forms: Vec<(String, usize)> = forms.into_iter().collect();
            forms.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| a.0.len().cmp(&b.0.len()))
                    .then_with(|| a.0.cmp(&b.0))
            });
            (forms.remove(0).0, total)
        })
        .collect();
    let phrases = phrases(paragraphs, names);
    // A word that is mostly used inside a suggested phrase is that phrase.
    ranked.retain(|(word, count)| {
        !phrases.iter().any(|(phrase, phrase_count)| {
            phrase.split(' ').any(|part| part == word) && phrase_count * 2 >= *count
        })
    });
    ranked.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.0.len().cmp(&a.0.len()))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut out = phrases;
    out.extend(ranked);
    out.truncate(MAX_KEYWORDS);
    out
}

/// Two-word phrases the chapter repeats ("harbour lights"), most used first: neither word a
/// name, a function word or a very common narrative word, and no sentence break or comma
/// between them.
fn phrases(paragraphs: &[&str], names: &HashSet<String>) -> Vec<(String, usize)> {
    let usable = |lower: &str| {
        lower.chars().count() >= 3
            && !names.contains(lower)
            && !is_stopword(lower)
            && !COMMON_WORDS.contains(&lower)
            && lower.chars().all(char::is_alphabetic)
    };
    let mut counts: HashMap<String, usize> = HashMap::new();
    for paragraph in paragraphs {
        let words = words(paragraph);
        for pair in words.windows(2) {
            let (first, second) = (&pair[0], &pair[1]);
            if first.comma || second.initial {
                continue;
            }
            let (a, b) = (first.text.to_lowercase(), second.text.to_lowercase());
            if usable(&a) && usable(&b) && a != b {
                *counts.entry(format!("{a} {b}")).or_insert(0) += 1;
            }
        }
    }
    let mut ranked: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, count)| *count >= MIN_PHRASE)
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(MAX_PHRASES);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(suggestions: &[TagSuggestion], category: TagCategory) -> Vec<String> {
        suggestions
            .iter()
            .filter(|s| s.tag.category == category)
            .map(|s| s.tag.name.clone())
            .collect()
    }

    const STORY: &str = "\
Mara watched the dragon circle the keep. Voss stood beside her, silent.
\"The dragon is early,\" Mara said. \"It is never early.\" Voss nodded and Mara frowned.
Voss looked at Mara and smiled. The dragon landed on the wall, and the dragon roared.
\"Will you stay?\" Mara asked. Voss kissed Mara, and the dragon watched them both.
Monday came slowly. Suddenly the storm broke. Suddenly the dragon was gone. Suddenly, silence.
Mara loved the keep. Voss loved the dragon. The storm returned, and the storm raged.\nThe storm howled all night, and the storm did not break.";

    #[test]
    fn characters_are_the_capitalised_names_that_recur() {
        let found = auto_suggestions(STORY);
        let characters = names(&found, TagCategory::Character);
        assert!(characters.contains(&"Mara".to_owned()), "{characters:?}");
        assert!(characters.contains(&"Voss".to_owned()), "{characters:?}");
        // Mara is mentioned more often, so she comes first.
        assert_eq!(characters[0], "Mara");
        // Not names: a weekday, and a word that only ever opens sentences.
        assert!(!characters.contains(&"Monday".to_owned()));
        assert!(!characters.contains(&"Suddenly".to_owned()));
        assert!(!characters.contains(&"The".to_owned()));
    }

    #[test]
    fn every_suggestion_says_what_earned_it() {
        for suggestion in auto_suggestions(STORY) {
            assert!(!suggestion.matched.is_empty(), "{:?}", suggestion.tag);
            assert!(suggestion.matched[0].count >= 1);
        }
        let mara = auto_suggestions(STORY)
            .into_iter()
            .find(|s| s.tag.name == "Mara")
            .expect("Mara");
        assert_eq!(mara.matched[0].term, "mentions");
        assert!(mara.matched[0].count >= 5);
    }

    #[test]
    fn a_pair_who_share_romantic_scenes_is_written_with_a_slash() {
        let found = auto_suggestions(STORY);
        let relationships = names(&found, TagCategory::Relationship);
        assert_eq!(
            relationships,
            vec!["Mara/Voss".to_owned()],
            "{relationships:?}"
        );
        let pair = found
            .iter()
            .find(|s| s.tag.category == TagCategory::Relationship)
            .expect("a pair");
        assert!(pair.matched.iter().any(|m| m.term.starts_with("romantic")));
    }

    #[test]
    fn a_pair_with_no_romantic_cue_words_is_written_with_an_ampersand() {
        let text = "\
Mara and Voss walked the wall.
Mara told Voss about the dragon.
Voss asked Mara about the storm.
Mara and Voss watched the keep.";
        let found = auto_suggestions(text);
        let relationships = names(&found, TagCategory::Relationship);
        assert_eq!(
            relationships,
            vec!["Mara & Voss".to_owned()],
            "{relationships:?}"
        );
    }

    #[test]
    fn keywords_are_the_words_the_chapter_leans_on_without_names_or_filler() {
        let keywords = names(&auto_suggestions(STORY), TagCategory::Freeform);
        assert!(keywords.contains(&"Dragon".to_owned()), "{keywords:?}");
        assert!(keywords.contains(&"Storm".to_owned()), "{keywords:?}");
        // Names, function words and common narrative words stay out.
        for out in ["Mara", "Voss", "Said", "The", "Looked"] {
            assert!(!keywords.contains(&out.to_owned()), "{out} in {keywords:?}");
        }
    }

    #[test]
    fn a_title_in_front_of_a_name_does_not_become_a_second_character() {
        let text = "\
Captain Voss raised a hand. Voss was tired.
The crew obeyed Voss. Voss stood watch.
Mara asked Voss about the sea. Voss said nothing.";
        let characters = names(&auto_suggestions(text), TagCategory::Character);
        assert!(characters.contains(&"Voss".to_owned()), "{characters:?}");
        assert!(
            !characters.contains(&"Captain".to_owned()),
            "{characters:?}"
        );
    }

    #[test]
    fn a_name_that_is_also_an_ordinary_word_is_taken_for_the_word() {
        let text = "\
She felt hope. Hope was a bright thing, and her hope grew.
Then Hope laughed. Hope said hope was enough. Hope stayed.";
        let characters = names(&auto_suggestions(text), TagCategory::Character);
        assert!(!characters.contains(&"Hope".to_owned()), "{characters:?}");
    }

    #[test]
    fn the_same_text_always_gives_the_same_suggestions() {
        assert_eq!(auto_suggestions(STORY), auto_suggestions(STORY));
    }

    #[test]
    fn an_empty_or_nameless_chapter_gives_nothing_rather_than_noise() {
        assert!(auto_suggestions("").is_empty());
        assert!(auto_suggestions("a few words. nothing here. all lower case.").is_empty());
    }

    #[test]
    fn ratings_and_warnings_are_never_suggested() {
        for suggestion in auto_suggestions(STORY) {
            assert!(suggestion.tag.category.fixed_names().is_none());
        }
    }

    #[test]
    fn a_tag_the_vocabulary_already_has_is_not_offered_twice() {
        let vocabulary = vec![TagSuggestion {
            tag: Tag::new("dragon", TagCategory::Freeform),
            score: 2.0,
            matched: Vec::new(),
        }];
        let merged = merge_suggestions(vocabulary, auto_suggestions(STORY));
        let dragons = merged
            .iter()
            .filter(|s| s.tag.name.eq_ignore_ascii_case("dragon"))
            .count();
        assert_eq!(dragons, 1);
        // The writer's own entry comes first.
        assert_eq!(merged[0].tag.name, "dragon");
    }

    // ------------------------------------------------------------ full names and phrases

    fn names_of(suggestions: &[TagSuggestion], category: TagCategory) -> Vec<String> {
        suggestions
            .iter()
            .filter(|s| s.tag.category == category)
            .map(|s| s.tag.name.clone())
            .collect()
    }

    #[test]
    fn a_first_name_is_suggested_as_the_full_name_the_story_uses() {
        let story = "Mara Ellison came home late.\nLater, Mara Ellison slept.\n\
                     The rain fell on Mara.\nIt woke Mara at dawn.\nShe found Mara's coat.";
        let book = NameBook::from_text(story);
        assert_eq!(book.names(), ["Mara Ellison"]);
        // A chapter that only says "Mara".
        let chapter = "The rain fell on Mara.\nIt woke Mara at dawn.\nThey called Mara in.";
        let found = auto_suggestions_with(chapter, &book);
        let characters = names_of(&found, TagCategory::Character);
        assert_eq!(characters, ["Mara Ellison"]);
        let evidence = &found[0].matched[0];
        assert_eq!(evidence.count, 3);
        assert_eq!(evidence.term, "mentions (as Mara)");
    }

    #[test]
    fn a_surname_two_people_share_stays_as_it_is() {
        let story = "Then Mara Ellison spoke. Then Mara Ellison left.\n\
                     Then James Ellison spoke. Then James Ellison left.";
        let book = NameBook::from_text(story);
        assert_eq!(book.names().len(), 2);
        let chapter = "The letter was for Ellison.\nAnd Ellison read it.\nSo Ellison wept.";
        let characters = names_of(
            &auto_suggestions_with(chapter, &book),
            TagCategory::Character,
        );
        assert_eq!(characters, ["Ellison"], "ambiguous: not folded");
        // But each first name still finds its full name.
        let chapter = "Then Mara ran.\nAnd Mara hid.\nSo Mara waited.";
        let characters = names_of(
            &auto_suggestions_with(chapter, &book),
            TagCategory::Character,
        );
        assert_eq!(characters, ["Mara Ellison"]);
    }

    #[test]
    fn remembered_names_fill_in_but_never_override_the_story() {
        let mut book = NameBook::from_text("Then Mara Quinn spoke. Then Mara Quinn left.");
        book.remember(["Mara Ellison", "Tomas Reyes", "Solo"]);
        assert_eq!(book.names(), ["Mara Quinn", "Tomas Reyes"]);
        let chapter = "Then Tomas laughed.\nAnd Tomas sang.\nSo Tomas left.";
        let characters = names_of(
            &auto_suggestions_with(chapter, &book),
            TagCategory::Character,
        );
        assert_eq!(characters, ["Tomas Reyes"]);
    }

    #[test]
    fn pairings_use_full_names_and_count_any_form() {
        let mut book = NameBook::default();
        book.remember(["Mara Ellison", "Tomas Reyes"]);
        let chapter = "Then Mara kissed Tomas.\nAnd Mara held Tomas, her love.\n\
                       So Mara watched Tomas sleep.\nThen Mara and Tomas kissed again.";
        let found = auto_suggestions_with(chapter, &book);
        let pairs = names_of(&found, TagCategory::Relationship);
        assert_eq!(pairs, ["Mara Ellison/Tomas Reyes"]);
    }

    #[test]
    fn a_repeated_two_word_phrase_is_suggested_once_as_a_phrase() {
        let chapter = "The harbour lights blinked.\nShe watched the harbour lights.\n\
                       Below, the harbour lights shone.\nThe harbour was quiet.";
        let freeform = names_of(&auto_suggestions(chapter), TagCategory::Freeform);
        assert!(
            freeform.contains(&"Harbour Lights".to_owned()),
            "{freeform:?}"
        );
        assert!(
            !freeform.contains(&"Harbour".to_owned()),
            "folded into the phrase"
        );
    }

    #[test]
    fn an_opening_word_in_front_of_a_full_name_is_not_part_of_it() {
        let story =
            "Later Mara Ellison wrote.\nLater Mara Ellison wrote.\nLater Mara Ellison slept.";
        let book = NameBook::from_text(story);
        assert!(
            book.names().contains(&"Mara Ellison".to_owned()),
            "{:?}",
            book.names()
        );
        let characters = names_of(&auto_suggestions_with(story, &book), TagCategory::Character);
        assert_eq!(characters, ["Mara Ellison"]);
    }
}
