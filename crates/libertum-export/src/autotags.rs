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
//! * **Additional tags** are the words the chapter leans on most, after names, function words
//!   and a list of very common narrative words are set aside. There is no corpus to compare
//!   against, so this list is the noisiest of the three.
//!
//! The fandom cannot be told from a chapter's text and is never suggested, and neither are
//! ratings or warnings, which the writer chooses on AO3.

use std::collections::{HashMap, HashSet};

use libertum_model::Document;

use crate::tags::{Tag, TagCategory, TagMatch, TagSuggestion, is_stopword, stem};
use crate::{Chapter, chapter_text};

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
    let paragraphs: Vec<&str> = text
        .split('\n')
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .collect();
    let scan = scan_names(&paragraphs);
    let characters = characters(&scan);
    let relationships = relationships(&paragraphs, &characters);
    let name_words: HashSet<String> = characters
        .iter()
        .flat_map(|(name, _)| name.split(' ').map(str::to_lowercase))
        .chain(scan.all_name_words())
        .collect();
    let keywords = keywords(&paragraphs, &name_words);

    let mut out = Vec::new();
    for (name, count) in &characters {
        out.push(suggestion(
            name.clone(),
            TagCategory::Character,
            *count as f32,
            vec![TagMatch {
                term: "mentions".to_owned(),
                count: *count,
            }],
        ));
    }
    out.extend(relationships);
    for (word, count) in keywords {
        out.push(suggestion(
            capitalise(&word),
            TagCategory::Freeform,
            count as f32,
            vec![TagMatch { term: word, count }],
        ));
    }
    out
}

/// [`auto_suggestions`] for one chapter's text.
pub fn auto_suggestions_for_chapter(document: &Document, chapter: &Chapter) -> Vec<TagSuggestion> {
    auto_suggestions(&chapter_text(document, chapter))
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
        lower: HashSet::new(),
    };
    for paragraph in paragraphs {
        let words = words(paragraph);
        let mut run: Vec<&str> = Vec::new();
        let flush_run = |run: &mut Vec<&str>, scan: &mut NameScan| {
            if run.len() >= 2 {
                *scan.runs.entry(run.join(" ")).or_insert(0) += 1;
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
                flush_run(&mut run, &mut scan);
                continue;
            }
            // A new sentence starts a new run: "...Voss. Mara..." is two names, not one.
            if word.initial {
                flush_run(&mut run, &mut scan);
                *scan.initial.entry(word.text.clone()).or_insert(0) += 1;
                if word.comma {
                    *scan.initial_comma.entry(word.text.clone()).or_insert(0) += 1;
                }
            } else {
                *scan.middle.entry(word.text.clone()).or_insert(0) += 1;
            }
            if run.len() == MAX_NAME_WORDS {
                flush_run(&mut run, &mut scan);
            }
            run.push(&word.text);
        }
        flush_run(&mut run, &mut scan);
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
    found.truncate(MAX_CHARACTERS);
    found
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

fn relationships(paragraphs: &[&str], characters: &[(String, usize)]) -> Vec<TagSuggestion> {
    let pool: Vec<&(String, usize)> = characters.iter().take(PAIR_POOL).collect();
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
            .filter(|(_, (name, _))| mentions(paragraph, name))
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
            let name = format!("{}{separator}{}", pool[a].0, pool[b].0);
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
    ranked.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.0.len().cmp(&a.0.len()))
            .then_with(|| a.0.cmp(&b.0))
    });
    ranked.truncate(MAX_KEYWORDS);
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
}
