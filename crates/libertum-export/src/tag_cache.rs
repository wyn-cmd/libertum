//! The local tag library: tags the writer has used before, tags imported from a file, and a
//! bundled starter list of common AO3 additional tags. All of it lives on this computer and
//! nothing is fetched from anywhere.
//!
//! - **Learned** tags are the ones the writer ticked and copied (Copy tags). Characters and
//!   pairings among them give the name book its full names in later chapters and stories.
//! - **Imported** tags come from a file the writer chooses: one tag per line (optionally
//!   `Character: Name`), or a CSV with `name` and `type` columns (and, when present, a count
//!   column to keep the most used, and a `canonical` column to keep canonical tags only).
//! - The **starter list** (`data/ao3-starter-tags.txt`) is hand-written from general
//!   knowledge and meant to be reviewed: `Tag Name | cue, cue, ...`, where the cues are words a
//!   chapter with that tag tends to use. A tag with no cues is never suggested from text.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::io::BufRead;

use serde::{Deserialize, Serialize};

use crate::tags::{Tag, TagCategory};

/// Most tags the library keeps; an import past it keeps the most used.
pub const MAX_CACHED_TAGS: usize = 20_000;

/// The bundled starter list, as written in the repository.
pub const STARTER_LIST: &str = include_str!("../data/ao3-starter-tags.txt");

/// Where a library tag came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheSource {
    Learned,
    Imported,
}

/// One tag in the library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CachedTag {
    pub tag: Tag,
    pub source: CacheSource,
    /// Times the writer copied it (learned tags).
    #[serde(default)]
    pub uses: u32,
}

/// The writer's tag library (learned and imported tags).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TagCache {
    pub tags: Vec<CachedTag>,
}

fn same(a: &Tag, b: &Tag) -> bool {
    a.category == b.category && a.name.eq_ignore_ascii_case(&b.name)
}

impl TagCache {
    /// Remembers tags the writer used. The rating and the warnings are AO3's fixed choices
    /// and are not kept. Returns how many were new.
    pub fn learn(&mut self, tags: &[Tag]) -> usize {
        let mut added = 0;
        for tag in tags {
            let name = tag.name.trim();
            if name.is_empty() || tag.category.fixed_names().is_some() {
                continue;
            }
            match self.tags.iter_mut().find(|cached| same(&cached.tag, tag)) {
                Some(cached) => {
                    cached.uses = cached.uses.saturating_add(1);
                    cached.source = CacheSource::Learned;
                }
                None => {
                    self.tags.push(CachedTag {
                        tag: Tag::new(name, tag.category),
                        source: CacheSource::Learned,
                        uses: 1,
                    });
                    added += 1;
                }
            }
        }
        added
    }

    /// Adds imported tags that the library does not have yet, up to [`MAX_CACHED_TAGS`].
    /// Returns how many were added.
    pub fn import(&mut self, tags: Vec<Tag>) -> usize {
        let mut added = 0;
        for tag in tags {
            if self.tags.len() >= MAX_CACHED_TAGS {
                break;
            }
            if tag.category.fixed_names().is_some()
                || self.tags.iter().any(|cached| same(&cached.tag, &tag))
            {
                continue;
            }
            self.tags.push(CachedTag {
                tag,
                source: CacheSource::Imported,
                uses: 0,
            });
            added += 1;
        }
        added
    }

    /// Forgets every learned tag (imported ones stay).
    pub fn forget_learned(&mut self) {
        self.tags
            .retain(|cached| cached.source != CacheSource::Learned);
    }

    /// How many tags came from each source: (learned, imported).
    pub fn counts(&self) -> (usize, usize) {
        let learned = self
            .tags
            .iter()
            .filter(|c| c.source == CacheSource::Learned)
            .count();
        (learned, self.tags.len() - learned)
    }

    /// The library's names in one category.
    pub fn names(&self, category: TagCategory) -> impl Iterator<Item = &str> {
        self.tags
            .iter()
            .filter(move |cached| cached.tag.category == category)
            .map(|cached| cached.tag.name.as_str())
    }

    /// The library's tags that are ranked against a chapter's words: fandoms and additional
    /// tags (characters and pairings are found by name instead).
    pub fn rankable(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .filter(|cached| {
                matches!(
                    cached.tag.category,
                    TagCategory::Fandom | TagCategory::Freeform
                )
            })
            .map(|cached| cached.tag.clone())
            .collect()
    }
}

/// The starter list's tags, with their cue words. Comment and blank lines are skipped.
pub fn starter_tags() -> Vec<Tag> {
    parse_starter(STARTER_LIST)
}

fn parse_starter(text: &str) -> Vec<Tag> {
    let mut seen: HashSet<String> = HashSet::new();
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let (name, cues) = line.split_once('|').unwrap_or((line, ""));
            let name = name.trim();
            if name.is_empty() || !seen.insert(name.to_lowercase()) {
                return None;
            }
            let mut tag = Tag::new(name, TagCategory::Freeform);
            tag.cues = cues
                .split(',')
                .map(|cue| cue.trim().to_lowercase())
                .filter(|cue| !cue.is_empty())
                .collect();
            Some(tag)
        })
        .collect()
}

/// The category a word in an import names ("Additional Tags", "freeform", "Character" ...).
fn category_named(word: &str) -> Option<TagCategory> {
    let word = word.trim().to_lowercase();
    let word = word.trim_end_matches('s');
    Some(match word {
        "fandom" => TagCategory::Fandom,
        "character" => TagCategory::Character,
        "relationship" | "pairing" | "ship" => TagCategory::Relationship,
        "freeform" | "additional" | "additional tag" | "tag" => TagCategory::Freeform,
        _ => return None,
    })
}

/// An imported row while the most used are kept: (count, file order reversed, name,
/// category code).
type Ranked = (u64, Reverse<usize>, String, u8);

/// Splits one CSV line, honouring double-quoted fields with doubled quotes inside.
fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    out.push(field);
    out
}

/// Reads a tag file: a CSV with `name` and `type` (or `category`) columns, or one tag per
/// line (`Category: Name`, or a bare name for an additional tag; `#` starts a comment).
/// Ratings, warnings and other kinds AO3 uses are skipped. At most [`MAX_CACHED_TAGS`] are
/// returned: from a CSV with a count column (`cached_count`, `count`, `uses`), the most used.
pub fn parse_tag_import(reader: impl BufRead) -> Vec<Tag> {
    let mut lines = reader.lines().map_while(Result::ok);
    let Some(first) = lines.next() else {
        return Vec::new();
    };
    let first = first.trim_start_matches('\u{feff}').to_owned();
    let header: Vec<String> = csv_fields(&first)
        .into_iter()
        .map(|h| h.trim().to_lowercase())
        .collect();
    let column = |names: &[&str]| header.iter().position(|h| names.contains(&h.as_str()));
    let (name_col, type_col) = (column(&["name", "tag"]), column(&["type", "category"]));
    let (Some(name_col), Some(type_col)) = (name_col, type_col) else {
        return parse_lines(std::iter::once(first).chain(lines));
    };
    let count_col = column(&["cached_count", "count", "uses", "works"]);
    let canonical_col = column(&["canonical"]);
    // Keep the most used: a min-heap of (count, order, tag) bounded at the cap.
    let mut heap: BinaryHeap<Reverse<Ranked>> = BinaryHeap::new();
    let mut seen: HashSet<(u8, String)> = HashSet::new();
    for (order, line) in lines.enumerate() {
        let fields = csv_fields(&line);
        let field = |i: usize| fields.get(i).map(|f| f.trim()).unwrap_or("");
        if canonical_col.is_some_and(|i| {
            !matches!(field(i).to_lowercase().as_str(), "true" | "t" | "1" | "yes")
        }) {
            continue;
        }
        let Some(category) = category_named(field(type_col)) else {
            continue;
        };
        let name = field(name_col);
        if name.is_empty() || name.len() > 200 {
            continue;
        }
        let code = category as u8;
        if !seen.insert((code, name.to_lowercase())) {
            continue;
        }
        let count = count_col
            .and_then(|i| field(i).parse::<u64>().ok())
            .unwrap_or(0);
        heap.push(Reverse((count, Reverse(order), name.to_owned(), code)));
        if heap.len() > MAX_CACHED_TAGS {
            heap.pop();
        }
    }
    let mut kept: Vec<(u64, usize, String, u8)> = heap
        .into_iter()
        .map(|Reverse((count, Reverse(order), name, code))| (count, order, name, code))
        .collect();
    kept.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    kept.into_iter()
        .filter_map(|(_, _, name, code)| {
            let category = [
                TagCategory::Rating,
                TagCategory::Warning,
                TagCategory::Fandom,
                TagCategory::Relationship,
                TagCategory::Character,
                TagCategory::Freeform,
            ]
            .into_iter()
            .find(|c| *c as u8 == code)?;
            Some(Tag::new(name, category))
        })
        .collect()
}

fn parse_lines(lines: impl Iterator<Item = String>) -> Vec<Tag> {
    let mut out: Vec<Tag> = Vec::new();
    let mut seen: HashSet<(TagCategory, String)> = HashSet::new();
    for line in lines {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (category, name) = match line.split_once(':') {
            Some((prefix, rest)) => match category_named(prefix) {
                Some(category) => (category, rest.trim()),
                None => (TagCategory::Freeform, line),
            },
            None => (TagCategory::Freeform, line),
        };
        if name.is_empty() || name.len() > 200 || !seen.insert((category, name.to_lowercase())) {
            continue;
        }
        out.push(Tag::new(name, category));
        if out.len() >= MAX_CACHED_TAGS {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learning_counts_uses_and_skips_fixed_choices() {
        let mut cache = TagCache::default();
        let tags = [
            Tag::new("Found Family", TagCategory::Freeform),
            Tag::new("Mara Ellison", TagCategory::Character),
            Tag::new("General Audiences", TagCategory::Rating),
        ];
        assert_eq!(cache.learn(&tags), 2);
        assert_eq!(
            cache.learn(&[Tag::new("found family", TagCategory::Freeform)]),
            0
        );
        assert_eq!(cache.tags[0].uses, 2);
        assert_eq!(cache.counts(), (2, 0));
        assert_eq!(
            cache.names(TagCategory::Character).collect::<Vec<_>>(),
            ["Mara Ellison"]
        );
        assert_eq!(
            cache.rankable().len(),
            1,
            "characters are found by name, not ranked"
        );
        cache.import(vec![Tag::new("Slow Burn", TagCategory::Freeform)]);
        cache.forget_learned();
        assert_eq!(cache.counts(), (0, 1));
    }

    #[test]
    fn a_plain_list_takes_category_prefixes_and_comments() {
        let text = "# my tags\nFluff\nCharacter: Mara Ellison\nRelationship: Mara/Tomas\n\
                    Rating: Mature\nfluff\nAngst: lots of it\n\n";
        let tags = parse_tag_import(text.as_bytes());
        let shown: Vec<(TagCategory, &str)> =
            tags.iter().map(|t| (t.category, t.name.as_str())).collect();
        assert_eq!(
            shown,
            [
                (TagCategory::Freeform, "Fluff"),
                (TagCategory::Character, "Mara Ellison"),
                (TagCategory::Relationship, "Mara/Tomas"),
                (TagCategory::Freeform, "Rating: Mature"),
                (TagCategory::Freeform, "Angst: lots of it"),
            ]
        );
    }

    #[test]
    fn a_csv_keeps_canonical_known_kinds_most_used_first() {
        let csv = "id,type,name,canonical,cached_count,merger_id\n\
                   1,Freeform,Fluff,true,900,\n\
                   2,Freeform,fluffy fluff,false,5,1\n\
                   3,Rating,Explicit,true,999,\n\
                   4,Freeform,\"Angst, Lots\",true,1200,\n\
                   5,Character,Mara Ellison,true,3,\n\
                   6,Media,Books,true,9999,\n";
        let tags = parse_tag_import(csv.as_bytes());
        let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Angst, Lots", "Fluff", "Mara Ellison"]);
    }

    #[test]
    fn the_starter_list_parses_with_cues_and_no_duplicates() {
        let tags = parse_starter("# c\nHurt/Comfort | bandage, Fever\nCrack |\nhurt/comfort | x\n");
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].cues, ["bandage", "fever"]);
        assert!(tags[1].cues.is_empty());
        let bundled = starter_tags();
        assert!(bundled.len() >= 100, "the bundled list is there");
        assert!(bundled.iter().all(|t| t.category == TagCategory::Freeform));
    }
}
