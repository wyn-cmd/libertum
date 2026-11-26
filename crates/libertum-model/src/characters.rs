//! The characters a writer has saved for a story: a name, an age, some details, and the points
//! of the story checklist each one belongs to.
//!
//! A character linked to a point lends the point their name as an extra keyword, so a chapter
//! that mentions the character counts towards the point (see [`Characters::lend_names`]). The
//! link is kept on the character, and the checklist is never edited: unlinking, or deleting
//! the character, takes the keyword away again.
//!
//! Names follow AO3's tag style, `Full Name` or `Full Name (Fandom)`, so that what the writer
//! saves can be pasted into AO3's character field. The fandom part is dropped when the name is
//! used as a keyword.

use serde::{Deserialize, Serialize};

use crate::checklist::{Checklist, ChecklistItem};

/// One saved character.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Character {
    pub id: u64,
    /// As AO3 would show it: `Hermione Granger` or `Hermione Granger (Harry Potter)`.
    pub name: String,
    /// Free text, so "17", "about thirty" and "ageless" all fit.
    pub age: String,
    pub details: String,
    /// Ids of the checklist points this character is linked to.
    pub plot_points: Vec<u64>,
}

impl Character {
    /// The name without a trailing `(Fandom)`, which is what appears in the prose.
    pub fn short_name(&self) -> &str {
        short_name(&self.name)
    }

    /// The fandom in a trailing `(Fandom)`, if the name has one.
    pub fn fandom(&self) -> Option<&str> {
        fandom_of(&self.name)
    }

    pub fn is_linked_to(&self, point: u64) -> bool {
        self.plot_points.contains(&point)
    }
}

/// `"Hermione Granger (Harry Potter)"` is `"Hermione Granger"`. Only a final parenthesis that
/// follows a space is taken as the fandom, so a name such as `"Ash (1)"` is cut too but
/// `"(Hi)"` alone is left whole.
pub fn short_name(name: &str) -> &str {
    split_fandom(name).0
}

/// `"Hermione Granger (Harry Potter)"` is `Some("Harry Potter")`.
pub fn fandom_of(name: &str) -> Option<&str> {
    split_fandom(name).1
}

fn split_fandom(name: &str) -> (&str, Option<&str>) {
    let name = name.trim();
    if let Some(body) = name.strip_suffix(')')
        && let Some(open) = body.rfind(" (")
    {
        let short = body[..open].trim_end();
        let fandom = body[open + 2..].trim();
        if !short.is_empty() && !fandom.is_empty() {
            return (short, Some(fandom));
        }
    }
    (name, None)
}

/// What kind of tag a fandom tag is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FandomTagKind {
    Relationship,
    Freeform,
}

/// A tag read from AO3's recent works in the story's fandom, kept so the tag suggestions can
/// offer it exactly as AO3 writes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FandomTag {
    pub name: String,
    pub kind: FandomTagKind,
    /// How many of the works read carried it.
    pub uses: usize,
    /// The fandom it was read for.
    pub fandom: String,
}

/// Why a character could not be saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharacterError {
    /// The name is empty.
    NoName,
    /// A different character already has this name (ignoring case and spacing).
    Duplicate,
}

/// Every saved character, in the order they were made, and the fandoms the writer has chosen
/// for the story.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Characters {
    items: Vec<Character>,
    next_id: u64,
    /// The fandoms the story is in, as AO3 names them. Imports are limited to these.
    fandoms: Vec<String>,
    /// Relationship and additional tags read from AO3's recent works in those fandoms.
    tags: Vec<FandomTag>,
}

/// Names compare without regard to case or runs of spaces.
fn key(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl Characters {
    pub fn items(&self) -> &[Character] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when there is nothing to save: no characters and no fandoms.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.fandoms.is_empty() && self.tags.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Character> {
        self.items.iter().find(|c| c.id == id)
    }

    pub fn fandoms(&self) -> &[String] {
        &self.fandoms
    }

    /// Replaces the story's fandoms (blank and repeated ones dropped, order kept).
    pub fn set_fandoms(&mut self, fandoms: impl IntoIterator<Item = String>) {
        let mut kept: Vec<String> = Vec::new();
        for fandom in fandoms {
            let fandom = fandom.split_whitespace().collect::<Vec<_>>().join(" ");
            if !fandom.is_empty() && !kept.iter().any(|f| key(f) == key(&fandom)) {
                kept.push(fandom);
            }
        }
        self.fandoms = kept;
        // Tags read for a fandom the story is no longer in are not offered.
        let fandoms = &self.fandoms;
        self.tags
            .retain(|tag| fandoms.iter().any(|f| key(f) == key(&tag.fandom)));
    }

    /// The tags read from AO3 for the story's fandoms.
    pub fn fandom_tags(&self) -> &[FandomTag] {
        &self.tags
    }

    /// Replaces what was read for `fandom` (the tags of other fandoms stay).
    pub fn set_fandom_tags(&mut self, fandom: &str, tags: Vec<FandomTag>) {
        self.tags.retain(|tag| key(&tag.fandom) != key(fandom));
        self.tags.extend(tags);
    }

    /// Whether a character with this name is already saved (not counting `except`).
    pub fn has_name(&self, name: &str, except: Option<u64>) -> bool {
        let wanted = key(name);
        self.items
            .iter()
            .any(|c| Some(c.id) != except && key(&c.name) == wanted)
    }

    fn check(&self, name: &str, except: Option<u64>) -> Result<(), CharacterError> {
        if name.trim().is_empty() {
            Err(CharacterError::NoName)
        } else if self.has_name(name, except) {
            Err(CharacterError::Duplicate)
        } else {
            Ok(())
        }
    }

    /// Saves a new character and returns their id.
    pub fn add(
        &mut self,
        name: &str,
        age: &str,
        details: &str,
        plot_points: Vec<u64>,
    ) -> Result<u64, CharacterError> {
        self.check(name, None)?;
        self.next_id = self
            .next_id
            .max(self.items.iter().map(|c| c.id + 1).max().unwrap_or(0));
        let id = self.next_id;
        self.next_id += 1;
        let mut character = Character {
            id,
            name: tidy(name),
            age: age.trim().to_owned(),
            details: details.trim().to_owned(),
            plot_points: Vec::new(),
        };
        character.plot_points = deduplicated(plot_points);
        self.items.push(character);
        Ok(id)
    }

    /// Adds `name` only if nobody has it yet; for importing. Returns whether it was added.
    pub fn add_name(&mut self, name: &str) -> bool {
        self.add(name, "", "", Vec::new()).is_ok()
    }

    /// Changes a character. Nothing changes when it fails.
    pub fn update(
        &mut self,
        id: u64,
        name: &str,
        age: &str,
        details: &str,
        plot_points: Vec<u64>,
    ) -> Result<(), CharacterError> {
        self.check(name, Some(id))?;
        if let Some(character) = self.items.iter_mut().find(|c| c.id == id) {
            character.name = tidy(name);
            character.age = age.trim().to_owned();
            character.details = details.trim().to_owned();
            character.plot_points = deduplicated(plot_points);
        }
        Ok(())
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|c| c.id != id);
    }

    /// Takes links to points that no longer exist out of every character.
    pub fn forget_points_except(&mut self, live: impl Fn(u64) -> bool) {
        for character in &mut self.items {
            character.plot_points.retain(|point| live(*point));
        }
    }

    /// The characters whose name contains `query` (ignoring case), in saved order. An empty
    /// query matches everyone.
    pub fn search(&self, query: &str) -> Vec<&Character> {
        let query = key(query);
        self.items
            .iter()
            .filter(|c| query.is_empty() || key(&c.name).contains(&query))
            .collect()
    }

    /// The characters linked to a point.
    pub fn linked_to(&self, point: u64) -> impl Iterator<Item = &Character> {
        self.items.iter().filter(move |c| c.is_linked_to(point))
    }

    /// A copy of `list` in which every point also lists the names of the characters linked to
    /// it as extra words, ready to be scored. The list itself is unchanged.
    pub fn lend_names(&self, list: &Checklist) -> Checklist {
        if self.items.iter().all(|c| c.plot_points.is_empty()) {
            return list.clone();
        }
        let mut lent = list.clone();
        for item in lent.items_mut() {
            let names: Vec<&str> = self.linked_to(item.id).map(Character::short_name).collect();
            if !names.is_empty() {
                add_aliases(item, &names);
            }
        }
        lent
    }
}

fn add_aliases(item: &mut ChecklistItem, names: &[&str]) {
    for name in names {
        if !item.aliases.trim().is_empty() {
            item.aliases.push_str(", ");
        }
        item.aliases.push_str(name);
    }
}

/// Collapses runs of spaces so `"Hermione   Granger "` is saved as `"Hermione Granger"`.
fn tidy(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn deduplicated(mut points: Vec<u64>) -> Vec<u64> {
    let mut seen = Vec::new();
    points.retain(|p| {
        let new = !seen.contains(p);
        seen.push(*p);
        new
    });
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checklist::{ChapterProfile, ChapterRef, ChecklistOrder, rank};

    #[test]
    fn a_fandom_in_brackets_is_split_off_the_name() {
        assert_eq!(
            short_name("Hermione Granger (Harry Potter)"),
            "Hermione Granger"
        );
        assert_eq!(
            fandom_of("Hermione Granger (Harry Potter)"),
            Some("Harry Potter")
        );
        assert_eq!(short_name("Mara"), "Mara");
        assert_eq!(fandom_of("Mara"), None);
        assert_eq!(short_name("(Hi)"), "(Hi)");
        assert_eq!(
            fandom_of("Rey (Star Wars: Sequel Trilogy)"),
            Some("Star Wars: Sequel Trilogy")
        );
        assert_eq!(short_name("  Ash  (Pokemon) "), "Ash");
    }

    #[test]
    fn characters_are_added_found_changed_and_removed() {
        let mut cast = Characters::default();
        let mara = cast
            .add("  Mara   Voss ", " 29 ", "A smuggler", vec![3, 3, 4])
            .unwrap();
        assert_eq!(cast.get(mara).unwrap().name, "Mara Voss");
        assert_eq!(cast.get(mara).unwrap().age, "29");
        assert_eq!(
            cast.get(mara).unwrap().plot_points,
            vec![3, 4],
            "no repeats"
        );

        assert_eq!(cast.add("", "", "", vec![]), Err(CharacterError::NoName));
        assert_eq!(
            cast.add("mara voss", "", "", vec![]),
            Err(CharacterError::Duplicate)
        );
        let jo = cast.add("Jo", "", "", vec![]).unwrap();
        assert_ne!(jo, mara);

        assert_eq!(
            cast.update(jo, "Mara  VOSS", "", "", vec![]),
            Err(CharacterError::Duplicate)
        );
        assert_eq!(
            cast.get(jo).unwrap().name,
            "Jo",
            "a failed change changes nothing"
        );
        cast.update(mara, "Mara Voss (Tides)", "30", "", vec![])
            .unwrap();
        assert_eq!(cast.get(mara).unwrap().fandom(), Some("Tides"));
        assert_eq!(
            cast.update(mara, "Mara Voss (Tides)", "31", "", vec![]),
            Ok(()),
            "own name is fine"
        );

        assert_eq!(cast.search("VOSS").len(), 1);
        assert_eq!(cast.search("").len(), 2);
        assert_eq!(cast.search("zzz").len(), 0);

        cast.remove(mara);
        assert!(cast.get(mara).is_none());
        let again = cast.add("New", "", "", vec![]).unwrap();
        assert!(again > jo, "ids are not reused");
    }

    #[test]
    fn fandoms_are_tidied_and_a_cast_with_only_fandoms_is_not_empty() {
        let mut cast = Characters::default();
        assert!(cast.is_empty());
        cast.set_fandoms([
            "Harry Potter".to_owned(),
            " harry   potter ".to_owned(),
            String::new(),
        ]);
        assert_eq!(cast.fandoms(), ["Harry Potter"]);
        assert!(!cast.is_empty(), "the fandom choice must be saved");
    }

    #[test]
    fn fandom_tags_belong_to_a_fandom_and_go_when_the_fandom_does() {
        let tag = |name: &str, fandom: &str| FandomTag {
            name: name.to_owned(),
            kind: FandomTagKind::Relationship,
            uses: 3,
            fandom: fandom.to_owned(),
        };
        let mut cast = Characters::default();
        cast.set_fandoms(["A".to_owned(), "B".to_owned()]);
        cast.set_fandom_tags("A", vec![tag("x/y", "A")]);
        cast.set_fandom_tags("B", vec![tag("p/q", "B")]);
        cast.set_fandom_tags("A", vec![tag("m/n", "A")]);
        let names: Vec<&str> = cast.fandom_tags().iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["p/q", "m/n"],
            "reading a fandom again replaces its tags only"
        );
        cast.set_fandoms(["B".to_owned()]);
        assert_eq!(cast.fandom_tags().len(), 1, "A's tags went with A");
        assert!(!cast.is_empty());
        let bytes = serde_json::to_vec(&cast).unwrap();
        assert_eq!(serde_json::from_slice::<Characters>(&bytes).unwrap(), cast);
    }

    #[test]
    fn links_to_points_that_are_gone_are_dropped() {
        let mut cast = Characters::default();
        let a = cast.add("A", "", "", vec![1, 2, 3]).unwrap();
        cast.forget_points_except(|p| p != 2);
        assert_eq!(cast.get(a).unwrap().plot_points, vec![1, 3]);
    }

    #[test]
    fn a_linked_character_makes_their_name_a_keyword_of_the_point() {
        let mut list = Checklist::default();
        let heist = list.add("The heist goes wrong").unwrap();
        let calm = list.add("A quiet morning").unwrap();
        let mut cast = Characters::default();
        cast.add("Mara Voss (Tides)", "", "", vec![heist]).unwrap();

        let lent = cast.lend_names(&list);
        assert_eq!(lent.get(heist).unwrap().aliases, "Mara Voss");
        assert_eq!(lent.get(calm).unwrap().aliases, "");
        assert_eq!(
            list.get(heist).unwrap().aliases,
            "",
            "the writer's list is untouched"
        );

        // A chapter that mentions her now counts towards the point.
        let chapter = ChapterRef::new(0, "One");
        let profile = ChapterProfile::from_paragraphs(["Mara Voss ran. Mara hid."]);
        let plain = rank(&list, &profile, &chapter, ChecklistOrder::LeastCovered);
        let boosted = rank(&lent, &profile, &chapter, ChecklistOrder::LeastCovered);
        let of = |rows: &[crate::checklist::Ranked], id| {
            rows.iter().find(|r| r.id == id).unwrap().score.mentions
        };
        assert_eq!(of(&plain, heist), 0);
        assert!(of(&boosted, heist) >= 2);
    }

    #[test]
    fn existing_aliases_are_kept_when_names_are_lent() {
        let mut list = Checklist::default();
        let id = list.add("Theft").unwrap();
        list.set_aliases(id, "took, stole".to_owned());
        let mut cast = Characters::default();
        cast.add("Jo", "", "", vec![id]).unwrap();
        cast.add("Al", "", "", vec![id]).unwrap();
        assert_eq!(
            cast.lend_names(&list).get(id).unwrap().aliases,
            "took, stole, Jo, Al"
        );
    }

    #[test]
    fn a_cast_round_trips_through_json_and_old_files_read_as_empty() {
        let mut cast = Characters::default();
        cast.set_fandoms(["Tides".to_owned()]);
        cast.add("Mara", "29", "Smuggler", vec![1]).unwrap();
        let bytes = serde_json::to_vec(&cast).unwrap();
        assert_eq!(serde_json::from_slice::<Characters>(&bytes).unwrap(), cast);
        assert_eq!(
            serde_json::from_slice::<Characters>(b"{}").unwrap(),
            Characters::default()
        );
    }
}
