//! The most recent works of a fandom on AO3, read for the tags they carry.
//!
//! AO3 has no API, and its `robots.txt` disallows its autocomplete and search paths, so this
//! reads only what a person can read: the first pages of a fandom tag's work list
//! (`/tags/<fandom>/works`, newest first), which `robots.txt` does not disallow. It keeps up to
//! [`MAX_WORKS`] works that are in that one fandom (a work with another fandom as well is a
//! crossover and is skipped) and counts the relationships, characters and additional tags they
//! carry, so the names come back exactly as AO3 writes them.
//!
//! AO3 serves a "bot servers are overloaded" page (HTTP 429) to clients it takes for robots; the
//! importer then falls back to the wiki sites. The page reading below follows AO3's work-listing
//! markup (`li.work`, `h5.fandoms`, `li.characters`, `li.relationships`, `li.freeforms`, each
//! tag in `a.tag`) and has only been tested on made-up pages, because AO3 has not let this
//! program see a real one.

use std::collections::HashMap;
use std::time::Duration;

use crate::mediawiki::encode;

pub const HOST: &str = "archiveofourown.org";

/// Works kept (not counting crossovers).
pub const MAX_WORKS: usize = 50;
/// Pages of 20 works read at most.
pub const MAX_PAGES: usize = 3;
/// The pause before each request to AO3, longer than for wikis: AO3 asks bots to be gentle.
pub const GAP: Duration = Duration::from_secs(5);
/// Most requests for one lookup: `robots.txt` and the pages.
pub const MAX_REQUESTS: usize = 1 + MAX_PAGES;

const MAX_CHARACTERS: usize = 200;
const MAX_RELATIONSHIPS: usize = 100;
const MAX_FREEFORMS: usize = 150;

/// A tag name in the form AO3 puts in an address: `/`, `&`, `.`, `?` and `#` are written as
/// `*s*`, `*a*`, `*d*`, `*q*` and `*h*`, and the rest is percent-encoded.
pub fn tag_path(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        match c {
            '/' => out.push_str("*s*"),
            '&' => out.push_str("*a*"),
            '.' => out.push_str("*d*"),
            '?' => out.push_str("*q*"),
            '#' => out.push_str("*h*"),
            other => out.push(other),
        }
    }
    encode(&out)
}

/// One page of a fandom's works, newest posted first.
pub fn works_url(fandom: &str, page: usize) -> String {
    format!(
        "https://{HOST}/tags/{}/works?page={page}&work_search%5Bsort_column%5D=created_at",
        tag_path(fandom)
    )
}

/// The tags of one work as its listing shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Work {
    pub fandoms: Vec<String>,
    pub relationships: Vec<String>,
    pub characters: Vec<String>,
    pub freeforms: Vec<String>,
}

/// A tag and how many of the works carry it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tagged {
    pub name: String,
    pub uses: usize,
}

/// What the recent works say about a fandom.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    /// Works counted.
    pub works: usize,
    /// The fandom's name as most of those works write it: AO3's own.
    pub fandom: String,
    pub characters: Vec<Tagged>,
    pub relationships: Vec<Tagged>,
    pub freeforms: Vec<Tagged>,
}

/// The works listed on one page.
pub fn parse_works(html: &str) -> Vec<Work> {
    let mut starts: Vec<usize> = Vec::new();
    for marker in ["id=\"work_", "id='work_"] {
        let mut from = 0;
        while let Some(found) = html[from..].find(marker) {
            starts.push(from + found);
            from += found + marker.len();
        }
    }
    starts.sort_unstable();
    let mut works = Vec::new();
    for (index, start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(html.len());
        let chunk = &html[*start..end];
        let work = Work {
            fandoms: fandoms_of(chunk),
            relationships: tags_of(chunk, "relationships"),
            characters: tags_of(chunk, "characters"),
            freeforms: tags_of(chunk, "freeforms"),
        };
        if !work.fandoms.is_empty() {
            works.push(work);
        }
    }
    works
}

/// The anchors of the `h5.fandoms` heading.
fn fandoms_of(chunk: &str) -> Vec<String> {
    let Some(at) = chunk.find("fandoms heading") else {
        return Vec::new();
    };
    let rest = &chunk[at..];
    let end = rest.find("</h5>").unwrap_or(rest.len());
    anchors(&rest[..end])
}

/// Every `<li class='kind'>` of a work, each holding one tag.
fn tags_of(chunk: &str, kind: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in [
        format!("<li class='{kind}'"),
        format!("<li class=\"{kind}\""),
    ] {
        let mut from = 0;
        while let Some(found) = chunk[from..].find(&marker) {
            let start = from + found;
            let rest = &chunk[start..];
            let end = rest.find("</li>").unwrap_or(rest.len());
            out.extend(anchors(&rest[..end]));
            from = start + end.max(1);
        }
    }
    out
}

/// The text of each `<a ...>text</a>` in `region`.
fn anchors(region: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = region[from..].find("<a ") {
        let open = from + found;
        let Some(gt) = region[open..].find('>') else {
            break;
        };
        let text_start = open + gt + 1;
        let Some(close) = region[text_start..].find("</a>") else {
            break;
        };
        let name = decode(&strip_tags(&region[text_start..text_start + close]));
        if !name.is_empty() {
            out.push(name);
        }
        from = text_start + close + 4;
    }
    out
}

fn strip_tags(text: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            other if !in_tag => out.push(other),
            _ => {}
        }
    }
    out
}

/// The few character references tag names use.
fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|s| *s <= 8) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" | "#x27" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix('#')
                .and_then(|n| {
                    n.strip_prefix(['x', 'X'])
                        .map_or_else(|| n.parse().ok(), |hex| u32::from_str_radix(hex, 16).ok())
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Counts the tags of `works`, leaving out crossovers (a work with more than one fandom) and
/// keeping the first [`MAX_WORKS`]. `None` when no work is left.
pub fn profile(works: &[Work]) -> Option<Profile> {
    let kept: Vec<&Work> = works
        .iter()
        .filter(|work| work.fandoms.len() == 1)
        .take(MAX_WORKS)
        .collect();
    if kept.is_empty() {
        return None;
    }
    let count = |pick: fn(&Work) -> &Vec<String>, limit: usize| -> Vec<Tagged> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for work in &kept {
            // A tag repeated inside one work still counts once.
            let mut seen: Vec<&str> = Vec::new();
            for tag in pick(work) {
                if !seen.contains(&tag.as_str()) {
                    seen.push(tag);
                    *counts.entry(tag).or_insert(0) += 1;
                }
            }
        }
        let mut tagged: Vec<Tagged> = counts
            .into_iter()
            .map(|(name, uses)| Tagged {
                name: name.to_owned(),
                uses,
            })
            .collect();
        tagged.sort_by(|a, b| b.uses.cmp(&a.uses).then_with(|| a.name.cmp(&b.name)));
        tagged.truncate(limit);
        tagged
    };
    let fandoms = count(|w| &w.fandoms, 1);
    Some(Profile {
        works: kept.len(),
        fandom: fandoms.first().map(|t| t.name.clone()).unwrap_or_default(),
        characters: count(|w| &w.characters, MAX_CHARACTERS),
        relationships: count(|w| &w.relationships, MAX_RELATIONSHIPS),
        freeforms: count(|w| &w.freeforms, MAX_FREEFORMS),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A page in the shape of AO3's work listing, made up for the tests.
    pub fn work_html(
        id: u32,
        fandoms: &[&str],
        rels: &[&str],
        chars: &[&str],
        frees: &[&str],
    ) -> String {
        let anchors = |names: &[&str], class: &str| -> String {
            names
                .iter()
                .map(|n| {
                    format!("<li class='{class}'><a class=\"tag\" href=\"/tags/x\">{n}</a></li>")
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        let fandom_links: String = fandoms
            .iter()
            .map(|n| format!("<a class=\"tag\" href=\"/tags/f\">{n}</a>"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "<li id=\"work_{id}\" class=\"work blurb group work-{id}\" role=\"article\">\
             <div class=\"header module\"><h4 class=\"heading\"><a href=\"/works/{id}\">Title {id}</a> by <a rel=\"author\" href=\"/users/u\">u</a></h4>\
             <h5 class=\"fandoms heading\"><span class=\"landmark\">Fandoms:</span> {fandom_links} &nbsp;</h5></div>\
             <ul class=\"tags commas\"><li class='warnings'><strong><a class=\"tag\" href=\"/tags/w\">No Archive Warnings Apply</a></strong></li> {} {} {}</ul></li>",
            anchors(rels, "relationships"),
            anchors(chars, "characters"),
            anchors(frees, "freeforms"),
        )
    }

    #[test]
    fn tag_names_are_escaped_the_way_ao3_writes_addresses() {
        assert_eq!(
            tag_path("Harry Potter - J. K. Rowling"),
            "Harry%20Potter%20-%20J%2Ad%2A%20K%2Ad%2A%20Rowling"
        );
        assert_eq!(
            tag_path("Fire & Ice/Water?"),
            "Fire%20%2Aa%2A%20Ice%2As%2AWater%2Aq%2A"
        );
        let url = works_url("Hades (Video Game)", 2);
        assert!(url.starts_with(
            "https://archiveofourown.org/tags/Hades%20%28Video%20Game%29/works?page=2"
        ));
        assert!(url.contains("sort_column%5D=created_at"));
    }

    #[test]
    fn a_listing_is_read_into_works_with_their_tags() {
        let html = format!(
            "<ol class=\"work index group\">{}{}</ol>",
            work_html(
                1,
                &["Hades (Video Game)"],
                &["Zagreus/Megaera"],
                &["Zagreus (Hades)", "Megaera (Hades)"],
                &["Fluff", "Tom &amp; Jerry"]
            ),
            work_html(
                2,
                &["Hades (Video Game)", "Other Show"],
                &[],
                &["Zagreus (Hades)"],
                &[]
            ),
        );
        let works = parse_works(&html);
        assert_eq!(works.len(), 2);
        assert_eq!(works[0].fandoms, ["Hades (Video Game)"]);
        assert_eq!(works[0].relationships, ["Zagreus/Megaera"]);
        assert_eq!(works[0].characters, ["Zagreus (Hades)", "Megaera (Hades)"]);
        assert_eq!(
            works[0].freeforms,
            ["Fluff", "Tom & Jerry"],
            "entities are decoded"
        );
        assert_eq!(works[1].fandoms.len(), 2);
        assert!(parse_works("<html>The bot servers are currently overloaded</html>").is_empty());
    }

    #[test]
    fn crossovers_are_left_out_and_tags_are_counted_once_per_work() {
        let mut works = Vec::new();
        for i in 0..4 {
            works.push(Work {
                fandoms: vec!["Hades (Video Game)".into()],
                characters: vec!["Zagreus".into(), "Zagreus".into()],
                relationships: if i < 2 {
                    vec!["Zagreus/Megaera".into()]
                } else {
                    vec![]
                },
                freeforms: vec!["Fluff".into()],
            });
        }
        works.push(Work {
            fandoms: vec!["Hades (Video Game)".into(), "Other".into()],
            characters: vec!["Crossover Carl".into()],
            ..Work::default()
        });
        let profile = profile(&works).unwrap();
        assert_eq!(profile.works, 4);
        assert_eq!(profile.fandom, "Hades (Video Game)");
        assert_eq!(
            profile.characters,
            [Tagged {
                name: "Zagreus".into(),
                uses: 4
            }],
            "once per work, and no crossover character"
        );
        assert_eq!(profile.relationships[0].uses, 2);
        assert!(
            profile
                .characters
                .iter()
                .all(|t| t.name != "Crossover Carl")
        );
        assert_eq!(super::profile(&[]), None);
    }

    #[test]
    fn at_most_fifty_works_are_counted() {
        let works: Vec<Work> = (0..70)
            .map(|i| Work {
                fandoms: vec!["F".into()],
                characters: vec![format!("C{i}")],
                ..Work::default()
            })
            .collect();
        let profile = profile(&works).unwrap();
        assert_eq!(profile.works, MAX_WORKS);
        assert_eq!(profile.characters.len(), MAX_WORKS);
    }
}
