//! Turning a wiki page title into a character name the way AO3 writes character tags:
//! `Firstname Lastname`, and `Name (Fandom)` when a title carries a qualifier.

/// Qualifiers that only say what kind of page it is. They are dropped.
const GENERIC_QUALIFIERS: &[&str] = &[
    "character",
    "fictional character",
    "characters",
    "film",
    "movie",
    "novel",
    "book",
    "tv series",
    "television series",
    "comics",
    "comic",
    "video game",
    "game",
    "person",
    "canon",
    "legends",
    "canon character",
];

/// Titles that open with one of these are pages about pages, not people.
const NOT_PEOPLE_PREFIXES: &[&str] = &[
    "list of",
    "lists of",
    "characters",
    "category",
    "timeline",
    "template",
    "file",
    "help",
    "user",
    "talk",
    "portal",
    "index",
    "main page",
];

/// The AO3-style name for a wiki page `title` in `fandom`, or `None` when the page is not a
/// person (a list, a subpage, a disambiguation page).
///
/// - `Granger, Hermione` becomes `Hermione Granger`.
/// - A qualifier that only names the kind of page (`(character)`) is dropped.
/// - Any other qualifier is replaced by the writer's fandom, `Rey (Star Wars)`, which is how AO3
///   tells two characters of one name apart.
pub fn ao3_name(title: &str, fandom: &str) -> Option<String> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() || title.chars().count() > 80 || title.contains('/') || title.contains('#')
    {
        return None;
    }
    let lower = title.to_lowercase();
    if NOT_PEOPLE_PREFIXES.iter().any(|prefix| {
        lower == *prefix
            || lower
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with(' ') || rest.starts_with(':'))
    }) {
        return None;
    }
    let (stem, qualifier) = match title.strip_suffix(')').and_then(|t| t.rsplit_once(" (")) {
        Some((stem, qualifier)) if !stem.trim().is_empty() => {
            (stem.trim().to_owned(), Some(qualifier.trim().to_owned()))
        }
        _ => (title.clone(), None),
    };
    // "Last, First" with exactly one comma.
    let stem = match stem.split_once(", ") {
        Some((last, first)) if !first.contains(',') && !last.is_empty() && !first.is_empty() => {
            format!("{first} {last}")
        }
        _ => stem,
    };
    if stem.contains(',') || stem.chars().all(|c| !c.is_alphabetic()) {
        return None;
    }
    let fandom = fandom.split_whitespace().collect::<Vec<_>>().join(" ");
    match qualifier {
        None => Some(stem),
        Some(q) if GENERIC_QUALIFIERS.contains(&q.to_lowercase().as_str()) => Some(stem),
        Some(q) if q.eq_ignore_ascii_case("disambiguation") => None,
        Some(_) if fandom.is_empty() => Some(stem),
        Some(_) => Some(format!("{stem} ({fandom})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_become_ao3_names() {
        let n = |t: &str| ao3_name(t, "Star Wars");
        assert_eq!(n("Hermione Granger").as_deref(), Some("Hermione Granger"));
        assert_eq!(n("Granger, Hermione").as_deref(), Some("Hermione Granger"));
        assert_eq!(n("Rey (character)").as_deref(), Some("Rey"));
        assert_eq!(n("Rey (Star Wars)").as_deref(), Some("Rey (Star Wars)"));
        assert_eq!(
            n("Rey (Sequel Trilogy)").as_deref(),
            Some("Rey (Star Wars)")
        );
        assert_eq!(n("  Mara   Jade ").as_deref(), Some("Mara Jade"));
        assert_eq!(ao3_name("Rey (Sequel)", "").as_deref(), Some("Rey"));
    }

    #[test]
    fn pages_that_are_not_people_are_left_out() {
        for title in [
            "List of Star Wars characters",
            "Characters",
            "Timeline of the war",
            "Rey/Relationships",
            "Rey (disambiguation)",
            "Template:Infobox",
            "Category:Characters",
            "",
            "1138",
            "Doe, John, Jr.",
        ] {
            assert_eq!(ao3_name(title, "Star Wars"), None, "{title:?}");
        }
        assert_eq!(
            ao3_name("Listless Larry", "X").as_deref(),
            Some("Listless Larry")
        );
        assert_eq!(
            ao3_name("Characterization Carl", "X").as_deref(),
            Some("Characterization Carl")
        );
    }
}
