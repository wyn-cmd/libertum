//! Local AO3 tag suggestions (PLAN Phase 11).
//!
//! The writer keeps a tag vocabulary in Settings. This module ranks that vocabulary
//! against one chapter with BM25 and says which words earned each place, so a suggestion
//! can always be explained. Nothing here talks to a network, and nothing is applied
//! automatically: the review UI decides what to keep.
//!
//! The vocabulary is the corpus and the chapter is the query, which is the way round that
//! makes BM25 useful here: a word that appears in many tag names ("love", "character") gets
//! a low inverse document frequency and stops crowding out the specific ones.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// AO3's tag categories. They are never mixed in a ranking or a list: a rating does not
/// compete with a character name for the same place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagCategory {
    Rating,
    Warning,
    Fandom,
    Relationship,
    Character,
    Freeform,
}

/// AO3's ratings, in the order its posting form offers them. The form takes the rating as a
/// choice, not as text, so these are names to pick from and remember, not text to copy.
/// Taken from AO3's own pages (the Terms of Service FAQ and the Tags FAQ, found by search on
/// 2026-11-21; the live form was not read, because AO3 rate-limited the request).
pub const AO3_RATINGS: [&str; 5] = [
    "Not Rated",
    "General Audiences",
    "Teen And Up Audiences",
    "Mature",
    "Explicit",
];

/// AO3's archive warnings, which its form takes as tick boxes. Same sources as
/// [`AO3_RATINGS`]. **"Underage Sex" is the name a 2024 AO3 guide gives; the older Tags FAQ
/// still says "Underage". Check it against the live form.**
pub const AO3_WARNINGS: [&str; 6] = [
    "Creator Chose Not To Use Archive Warnings",
    "Graphic Depictions Of Violence",
    "Major Character Death",
    "No Archive Warnings Apply",
    "Rape/Non-Con",
    "Underage Sex",
];

impl TagCategory {
    /// The names AO3 fixes for this category, or `None` for the categories whose tags are
    /// free text (fandoms, relationships, characters and additional tags).
    pub fn fixed_names(self) -> Option<&'static [&'static str]> {
        match self {
            TagCategory::Rating => Some(&AO3_RATINGS),
            TagCategory::Warning => Some(&AO3_WARNINGS),
            _ => None,
        }
    }

    /// `text` as AO3 spells it in this category's fixed list (ignoring case and surrounding
    /// space), or `None` when it is not one of them or the category has no fixed list.
    pub fn fixed_name(self, text: &str) -> Option<&'static str> {
        let text = text.trim();
        self.fixed_names()?
            .iter()
            .copied()
            .find(|name| name.eq_ignore_ascii_case(text))
    }

    /// Every category, in the order AO3's form presents them.
    pub const ALL: [TagCategory; 6] = [
        TagCategory::Rating,
        TagCategory::Warning,
        TagCategory::Fandom,
        TagCategory::Relationship,
        TagCategory::Character,
        TagCategory::Freeform,
    ];
}

/// One tag in the writer's local vocabulary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub category: TagCategory,
    /// Extra words that also suggest this tag, besides those of its name (the starter list's
    /// "bandage, fever ..." for Hurt/Comfort). Empty for the writer's own tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cues: Vec<String>,
}

impl Tag {
    pub fn new(name: impl Into<String>, category: TagCategory) -> Self {
        Self {
            name: name.into(),
            category,
            cues: Vec::new(),
        }
    }
}

/// One word that earned a suggestion, and how often the chapter uses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagMatch {
    pub term: String,
    pub count: usize,
}

/// A vocabulary tag ranked against one chapter.
#[derive(Clone, Debug, PartialEq)]
pub struct TagSuggestion {
    pub tag: Tag,
    /// The BM25 score. Only its order matters; it is shown so a writer can see why one
    /// suggestion came above another.
    pub score: f32,
    /// The chapter words that matched, most frequent first.
    pub matched: Vec<TagMatch>,
}

/// BM25's term-frequency saturation and length-normalisation constants. The usual values.
const K1: f32 = 1.2;
const B: f32 = 0.75;

/// How many suggestions the review UI asks for.
pub const SUGGESTION_LIMIT: usize = 12;

/// The words a tag name and a chapter are compared on: lowercased, split at anything that
/// is not a letter or a digit. An apostrophe splits a word (`don't` becomes `don` and `t`),
/// which still lets the same stem match on both sides. [`stem`] does the comparison.
pub fn terms(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Words that carry no meaning for a tag: matching "and" should never be the reason a
/// chapter is tagged "Teen And Up Audiences". Kept short and English, like the stemmer.
const STOPWORDS: &[&str] = &[
    "a", "about", "after", "again", "all", "also", "an", "and", "any", "are", "as", "at", "be",
    "been", "before", "but", "by", "can", "could", "did", "do", "does", "for", "from", "had",
    "has", "have", "he", "her", "his", "i", "if", "in", "into", "is", "it", "its", "just", "may",
    "might", "must", "no", "not", "of", "on", "or", "our", "out", "over", "she", "so", "than",
    "that", "the", "their", "then", "these", "they", "this", "those", "to", "up", "us", "was",
    "we", "were", "will", "with", "would", "you", "your",
];

/// Whether a word is too common to explain a tag.
pub fn is_stopword(term: &str) -> bool {
    STOPWORDS.contains(&term)
}

/// A light English stem, so a tag written "Dragons" meets a chapter that says "dragon",
/// and "Enemies to Lovers" meets "enemy" and "loved". Deliberately small and predictable:
/// plurals, `-ing` and `-ed`, a final `e`, and a doubled final consonant. A tag ranked a
/// little too high costs nothing; an unexplained match would.
pub fn stem(word: &str) -> String {
    if word.len() <= 3 {
        return word.to_owned();
    }
    let mut stem = word;
    if stem.ends_with("ies") && stem.len() > 4 {
        return format!("{}y", &stem[..stem.len() - 3]);
    }
    // Only cut two bytes off a word that ends in "es": they are ASCII, so the cut is on a
    // character boundary ("café" must not be cut inside the "é").
    let es_after = stem.ends_with("es")
        && ["s", "x", "z", "ch", "sh"]
            .iter()
            .any(|ending| stem[..stem.len() - 2].ends_with(ending));
    if es_after {
        stem = &stem[..stem.len() - 2];
    } else if stem.ends_with('s')
        && !stem.ends_with("ss")
        && !stem.ends_with("us")
        && !stem.ends_with("is")
    {
        stem = &stem[..stem.len() - 1];
    }
    if stem.ends_with("ing") && stem.len() > 5 {
        stem = &stem[..stem.len() - 3];
    } else if stem.ends_with("ed") && stem.len() > 4 {
        stem = &stem[..stem.len() - 2];
    }
    if stem.len() > 3 && stem.ends_with('e') {
        stem = &stem[..stem.len() - 1];
    }
    // "running" became "runn"; the same word in a tag may be "run".
    let bytes = stem.as_bytes();
    if stem.len() > 3
        && bytes[stem.len() - 1] == bytes[stem.len() - 2]
        && bytes[stem.len() - 1].is_ascii_alphabetic()
        && !matches!(bytes[stem.len() - 1], b'l' | b's' | b'z')
    {
        stem = &stem[..stem.len() - 1];
    }
    if stem.is_empty() {
        word.to_owned()
    } else {
        stem.to_owned()
    }
}

/// What one stem means in the chapter: how often it occurs and the most common spelling.
struct ChapterTerm {
    count: usize,
    surface: String,
}

/// Ranks the vocabulary against one chapter. Deterministic: equal scores fall back to
/// category order, then to the tag name, and at most `limit` suggestions are returned.
pub fn suggest_tags(text: &str, vocabulary: &[Tag], limit: usize) -> Vec<TagSuggestion> {
    suggest_tags_with(text, vocabulary, limit, 1, 1)
}

/// [`suggest_tags`], keeping only tags whose words the chapter uses at least `min_hits`
/// times in all, and at least `min_terms` different words of them: the starter list and the
/// library need firmer evidence than the writer's own vocabulary, or a single "kiss" (or the
/// "one" in "one by one" for Only One Bed) would suggest a dozen tags.
pub fn suggest_tags_with(
    text: &str,
    vocabulary: &[Tag],
    limit: usize,
    min_hits: usize,
    min_terms: usize,
) -> Vec<TagSuggestion> {
    if vocabulary.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut chapter: HashMap<String, ChapterTerm> = HashMap::new();
    for word in terms(text) {
        if is_stopword(&word) {
            continue;
        }
        let stem = stem(&word);
        let entry = chapter.entry(stem).or_insert_with(|| ChapterTerm {
            count: 0,
            surface: word.clone(),
        });
        entry.count += 1;
        // The spelling shown should be the one the chapter uses most, so the explanation
        // reads like the chapter and stays the same from run to run.
        if entry.count == 1 && word < entry.surface {
            entry.surface = word;
        }
    }
    if chapter.is_empty() {
        return Vec::new();
    }

    // Tag names are compared by stem too, so "Dragons" and "dragon" meet; function words
    // in a tag name ("Enemies to Lovers") are dropped so only its subject can match.
    // A tag's cue words count as part of its name.
    let names: Vec<Vec<String>> = vocabulary
        .iter()
        .map(|tag| {
            terms(&tag.name)
                .into_iter()
                .chain(tag.cues.iter().flat_map(|cue| terms(cue)))
                .filter(|term| !is_stopword(term))
                .map(|term| stem(&term))
                .collect()
        })
        .collect();
    // Document frequency: in how many tag names each stem appears, once per name.
    let mut frequency: HashMap<&str, usize> = HashMap::new();
    for name in &names {
        let distinct: HashSet<&str> = name.iter().map(String::as_str).collect();
        for term in distinct {
            *frequency.entry(term).or_default() += 1;
        }
    }
    let documents = vocabulary.len() as f32;
    let total: usize = names.iter().map(Vec::len).sum();
    // A vocabulary of empty names is possible while the writer is still typing; the
    // average of 1.0 keeps the length normalisation finite.
    let average = if total == 0 {
        1.0
    } else {
        total as f32 / documents
    };

    let mut suggestions: Vec<TagSuggestion> = Vec::new();
    for (index, tag) in vocabulary.iter().enumerate() {
        let name = &names[index];
        if name.is_empty() {
            continue;
        }
        let length = name.len() as f32;
        let mut score = 0.0;
        let mut matched: Vec<TagMatch> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for term in name {
            // A word repeated in a tag name ("Kiss Kiss") counts once: the tag either
            // speaks about it or it does not.
            if !seen.insert(term.as_str()) {
                continue;
            }
            let Some(hit) = chapter.get(term.as_str()) else {
                continue;
            };
            let frequency = frequency.get(term.as_str()).copied().unwrap_or(1).max(1) as f32;
            // Lucene's BM25 idf: always positive, so a word need not be rare to count.
            let idf = (1.0 + (documents - frequency + 0.5) / (frequency + 0.5)).ln();
            // The word appears once in the tag name; the chapter's count is the query
            // term frequency, which is what makes a repeated word weigh more.
            let saturation = (K1 + 1.0) / (1.0 + K1 * (1.0 - B + B * length / average));
            score += hit.count as f32 * idf * saturation;
            matched.push(TagMatch {
                term: hit.surface.clone(),
                count: hit.count,
            });
        }
        let hits: usize = matched.iter().map(|m| m.count).sum();
        if score <= 0.0 || hits < min_hits || matched.len() < min_terms {
            continue;
        }
        matched.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.term.cmp(&b.term)));
        suggestions.push(TagSuggestion {
            tag: tag.clone(),
            score,
            matched,
        });
    }

    suggestions.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.tag.category.cmp(&b.tag.category))
            .then_with(|| a.tag.name.to_lowercase().cmp(&b.tag.name.to_lowercase()))
            .then_with(|| a.tag.name.cmp(&b.tag.name))
    });
    suggestions.truncate(limit);
    suggestions
}

/// The ticked tags as one line for AO3's tag fields: comma separated, in category order, then
/// by name. The rating and the warnings are left out: AO3 takes them as choices on its form,
/// and pasted into a tag field they would only make stray tags.
pub fn tags_to_text(suggestions: &[TagSuggestion], selected: &[bool]) -> String {
    let mut chosen: Vec<&Tag> = suggestions
        .iter()
        .zip(selected)
        .filter(|(_, on)| **on)
        .map(|(suggestion, _)| &suggestion.tag)
        .filter(|tag| tag.category.fixed_names().is_none())
        .collect();
    chosen.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    chosen
        .iter()
        .map(|tag| tag.name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str, category: TagCategory) -> Tag {
        Tag::new(name, category)
    }

    #[test]
    fn words_are_lowercased_and_split_at_punctuation() {
        assert_eq!(
            terms("Dragon-Rider's \"Bond\""),
            vec!["dragon", "rider", "s", "bond"]
        );
        assert!(terms("—").is_empty());
    }

    #[test]
    fn stems_join_plurals_and_verb_endings() {
        assert_eq!(stem("dragons"), "dragon");
        assert_eq!(stem("dragon"), "dragon");
        assert_eq!(stem("enemies"), "enemy");
        assert_eq!(stem("loved"), "lov");
        assert_eq!(stem("love"), "lov");
        assert_eq!(stem("lovers"), "lover");
        assert_eq!(stem("running"), "run");
        assert_eq!(stem("boxes"), "box");
        assert_eq!(stem("wishes"), "wish");
        assert_eq!(stem("box"), "box");
        assert_eq!(stem("pass"), "pass");
    }

    #[test]
    fn a_tag_the_chapter_is_about_is_suggested_and_explained() {
        let vocabulary = vec![
            tag("Dragons", TagCategory::Character),
            tag("Slow Burn", TagCategory::Freeform),
        ];
        let text = "The dragon spread its wings. Another dragon answered. A third dragon. \
                    Nothing else happened.";
        let suggestions = suggest_tags(text, &vocabulary, SUGGESTION_LIMIT);
        assert_eq!(suggestions.len(), 1, "{suggestions:?}");
        assert_eq!(suggestions[0].tag.name, "Dragons");
        assert_eq!(suggestions[0].matched.len(), 1);
        assert_eq!(suggestions[0].matched[0].term, "dragon");
        assert_eq!(suggestions[0].matched[0].count, 3);
        assert!(suggestions[0].score > 0.0);
    }

    #[test]
    fn a_word_shared_by_many_tags_weighs_less_than_a_specific_one() {
        let vocabulary = vec![
            tag("Dragon", TagCategory::Character),
            tag("Love", TagCategory::Freeform),
            tag("First Love", TagCategory::Freeform),
            tag("Love Triangle", TagCategory::Freeform),
        ];
        // The chapter uses both words exactly twice, so only the document frequency can
        // separate them: "love" is in three tag names, "dragon" in one.
        let text = "love love dragon dragon";
        let suggestions = suggest_tags(text, &vocabulary, SUGGESTION_LIMIT);
        assert_eq!(suggestions[0].tag.name, "Dragon", "{suggestions:?}");
        let love = suggestions
            .iter()
            .find(|suggestion| suggestion.tag.name == "Love")
            .expect("Love is suggested");
        assert!(suggestions[0].score > love.score);
    }

    #[test]
    fn a_repeated_chapter_word_weighs_more_than_a_single_use() {
        let vocabulary = vec![tag("Dragon", TagCategory::Character)];
        let once = suggest_tags("a dragon", &vocabulary, SUGGESTION_LIMIT);
        let thrice = suggest_tags(
            "a dragon, a dragon, a dragon",
            &vocabulary,
            SUGGESTION_LIMIT,
        );
        assert!(thrice[0].score > once[0].score);
        assert_eq!(thrice[0].matched[0].count, 3);
    }

    #[test]
    fn a_word_repeated_in_a_tag_name_counts_once() {
        let vocabulary = vec![tag("Kiss Kiss", TagCategory::Freeform)];
        let suggestions = suggest_tags("kiss", &vocabulary, SUGGESTION_LIMIT);
        assert_eq!(suggestions[0].matched.len(), 1);
        assert_eq!(suggestions[0].matched[0].term, "kiss");
    }

    #[test]
    fn categories_are_carried_through_and_order_the_ties() {
        let vocabulary = vec![
            tag("Freeform Ghost", TagCategory::Freeform),
            tag("Ghost", TagCategory::Character),
        ];
        // Both names contain "ghost"; the shorter name wins BM25's length normalisation.
        let suggestions = suggest_tags("ghost", &vocabulary, SUGGESTION_LIMIT);
        assert_eq!(suggestions[0].tag.name, "Ghost");
        assert_eq!(suggestions[0].tag.category, TagCategory::Character);
        assert_eq!(suggestions[1].tag.category, TagCategory::Freeform);
    }

    #[test]
    fn function_words_never_explain_a_tag() {
        let vocabulary = vec![tag("Teen And Up Audiences", TagCategory::Rating)];
        assert!(
            suggest_tags("and and and the of to", &vocabulary, SUGGESTION_LIMIT).is_empty(),
            "a chapter full of function words is not about that rating"
        );
        // "to" in a tag name is dropped, but its subject still matches.
        let vocabulary = vec![tag("Enemies to Lovers", TagCategory::Relationship)];
        let suggestions = suggest_tags("enemies", &vocabulary, SUGGESTION_LIMIT);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].matched[0].term, "enemies");
    }

    #[test]
    fn the_same_text_and_vocabulary_rank_the_same_way_every_time() {
        let vocabulary = vec![
            tag("Rivalry", TagCategory::Freeform),
            tag("Enemies to Lovers", TagCategory::Freeform),
            tag("Rival Schools", TagCategory::Fandom),
        ];
        let text = "rivals and enemies, rivals again, rivalry";
        let first = suggest_tags(text, &vocabulary, SUGGESTION_LIMIT);
        let second = suggest_tags(text, &vocabulary, SUGGESTION_LIMIT);
        assert_eq!(first, second);
        assert_eq!(first.len(), 3, "{first:?}");
        let names: Vec<&str> = first.iter().map(|s| s.tag.name.as_str()).collect();
        // The same list, in the same order, every time: the order is fixed by score,
        // then by category, then by name.
        assert_eq!(names, vec!["Rival Schools", "Rivalry", "Enemies to Lovers"]);
    }

    #[test]
    fn the_limit_caps_the_list_without_reordering_it() {
        let vocabulary: Vec<Tag> = (0..20)
            .map(|index| tag(&format!("ghost {index}"), TagCategory::Freeform))
            .collect();
        let all = suggest_tags("ghost", &vocabulary, 100);
        let capped = suggest_tags("ghost", &vocabulary, 5);
        assert_eq!(all.len(), 20);
        assert_eq!(capped.len(), 5);
        assert_eq!(&all[..5], &capped[..]);
    }

    #[test]
    fn a_chapter_or_vocabulary_with_no_overlap_suggests_nothing() {
        let vocabulary = vec![tag("Dragon", TagCategory::Character)];
        assert!(suggest_tags("a quiet village", &vocabulary, SUGGESTION_LIMIT).is_empty());
        assert!(suggest_tags("", &vocabulary, SUGGESTION_LIMIT).is_empty());
        assert!(suggest_tags("dragon", &[], SUGGESTION_LIMIT).is_empty());
        let blank = vec![tag("   ", TagCategory::Freeform)];
        assert!(suggest_tags("dragon", &blank, SUGGESTION_LIMIT).is_empty());
    }

    #[test]
    fn selected_tags_are_written_in_category_order_and_comma_separated() {
        let suggestions = vec![
            TagSuggestion {
                tag: tag("Slow Burn", TagCategory::Freeform),
                score: 2.0,
                matched: Vec::new(),
            },
            TagSuggestion {
                tag: tag("Rivals", TagCategory::Relationship),
                score: 1.0,
                matched: Vec::new(),
            },
            TagSuggestion {
                tag: tag("Ghost", TagCategory::Character),
                score: 0.5,
                matched: Vec::new(),
            },
        ];
        assert_eq!(
            tags_to_text(&suggestions, &[true, true, true]),
            "Rivals, Ghost, Slow Burn"
        );
        assert_eq!(
            tags_to_text(&suggestions, &[true, false, false]),
            "Slow Burn"
        );
        assert_eq!(tags_to_text(&suggestions, &[false, false, false]), "");
    }

    #[test]
    fn rating_and_warning_names_are_fixed_and_the_rest_are_free_text() {
        assert_eq!(TagCategory::Rating.fixed_names().map(<[_]>::len), Some(5));
        assert_eq!(TagCategory::Warning.fixed_names().map(<[_]>::len), Some(6));
        for category in [
            TagCategory::Fandom,
            TagCategory::Relationship,
            TagCategory::Character,
            TagCategory::Freeform,
        ] {
            assert!(category.fixed_names().is_none());
        }
        // Case and spacing are forgiven, and the canonical spelling comes back.
        assert_eq!(
            TagCategory::Rating.fixed_name("  teen and up audiences "),
            Some("Teen And Up Audiences")
        );
        assert_eq!(TagCategory::Rating.fixed_name("Teen"), None);
        assert_eq!(
            TagCategory::Warning.fixed_name("MAJOR CHARACTER DEATH"),
            Some("Major Character Death")
        );
        // A rating is not a warning.
        assert_eq!(TagCategory::Warning.fixed_name("Explicit"), None);
        assert_eq!(TagCategory::Freeform.fixed_name("Explicit"), None);
    }

    #[test]
    fn the_combined_line_leaves_out_the_rating_and_the_warnings() {
        let suggestion = |name: &str, category| TagSuggestion {
            tag: Tag::new(name, category),
            score: 1.0,
            matched: Vec::new(),
        };
        let suggestions = vec![
            suggestion("Mature", TagCategory::Rating),
            suggestion("Major Character Death", TagCategory::Warning),
            suggestion("Ghost", TagCategory::Character),
            suggestion("Slow Burn", TagCategory::Freeform),
        ];
        assert_eq!(
            tags_to_text(&suggestions, &[true; 4]),
            "Ghost, Slow Burn",
            "AO3 takes the rating and warnings as choices, not as tags"
        );
    }

    #[test]
    fn stemming_never_cuts_inside_an_accented_letter() {
        for word in [
            "café",
            "cafés",
            "née",
            "naïve",
            "protégé",
            "fiancée",
            "résumés",
        ] {
            let _ = stem(word);
        }
        assert_eq!(stem("cafés"), "café");
    }

    #[test]
    fn a_generic_tag_needs_more_than_one_common_word() {
        let mut bed = Tag::new("Only One Bed", TagCategory::Freeform);
        bed.cues = vec!["inn".into(), "share".into()];
        let vocabulary = [bed];
        let text = "One by one the lights went out. One more. One.";
        assert!(suggest_tags_with(text, &vocabulary, 5, 3, 2).is_empty());
        let text = "The inn had one bed. They would share the bed. One bed only.";
        assert_eq!(suggest_tags_with(text, &vocabulary, 5, 3, 2).len(), 1);
    }
}
