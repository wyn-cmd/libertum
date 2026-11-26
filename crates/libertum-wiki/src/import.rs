//! Looking up a fandom's characters on MediaWiki sites, politely.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::ao3;
use crate::mediawiki::{self, ApiError};
use crate::names::ao3_name;
use crate::robots::Robots;
use crate::sites::{Site, SiteKind};

/// What every request says it is.
pub const USER_AGENT: &str = concat!(
    "Libertum/",
    env!("CARGO_PKG_VERSION"),
    " (desktop writing app; bot; +https://github.com/wyn-cmd/libertum)"
);

/// The name a site's `robots.txt` can address this program by.
pub const ROBOTS_TOKEN: &str = "libertum";

/// The pause before every request after the first. Requests are made one at a time.
pub const REQUEST_GAP: Duration = Duration::from_millis(1100);

/// Subcategories looked into under the main one.
const MAX_SUBCATEGORIES: usize = 30;
/// Continuation pages read for one category (500 names each).
const MAX_PAGES_PER_CATEGORY: usize = 4;
/// Names kept in all.
const MAX_CANDIDATES: usize = 5_000;

/// One answer from a site.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("{0}")]
    Network(String),
    /// The site sent the request somewhere else (another host, or a plain http address).
    #[error("the site redirected to another address")]
    Redirected,
    #[error("the answer was too large")]
    TooLarge,
}

/// How requests are made. The app's implementation uses curl: https only, a size limit, a
/// time limit, no redirects to another host, and `USER_AGENT`.
pub trait Fetcher {
    fn get(&mut self, url: &str) -> Result<Response, FetchError>;
    /// Waits `duration`.
    fn pause(&mut self, duration: Duration);
    /// True once the writer has pressed Cancel.
    fn cancelled(&self) -> bool {
        false
    }
}

/// What to look for and where.
#[derive(Clone, Debug)]
pub struct Request {
    /// The fandom as AO3 names it. Every name that comes back is for this fandom.
    pub fandom: String,
    /// The fandom's wiki name on farms that give each wiki a subdomain.
    pub slug: String,
    /// The sites to try, in order; the first that gives characters wins.
    pub sites: Vec<Site>,
    /// Category names to try on each site (`Characters`), in order.
    pub categories: Vec<String>,
    /// Most requests, `robots.txt` included.
    pub max_requests: usize,
}

/// One character name the import found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// In AO3 form.
    pub name: String,
    /// The fandom it was looked up for. A document with one fandom is only ever shown its own.
    pub fandom: String,
    /// The site it came from.
    pub site: String,
    /// The page title, as the wiki has it.
    pub title: String,
    /// How many of AO3's recent works carry this character; 0 when it did not come from AO3.
    pub uses: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The site's `robots.txt` does not allow the API.
    RobotsDisallow,
    /// `robots.txt` could not be read (a server error or no answer), so nothing is asked.
    RobotsUnreachable,
    /// No such wiki.
    NotFound,
    NotMediaWiki,
    Refused(String),
    /// The wiki exists but none of the categories held a character.
    NoCharacters,
    NoWikiName,
    Network(String),
    /// The site is holding automated clients back (HTTP 429 or 503, a waiting room, `maxlag`).
    Blocked,
    /// A page came back, but not in the form this program reads.
    UnexpectedPage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skip {
    pub site: String,
    pub reason: SkipReason,
}

/// Why the whole run ended early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Cancelled,
    /// A site asked for slower requests (429, 503 or `maxlag`); nothing more is sent.
    SlowDown,
    OutOfRequests,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The site the names came from, if any did.
    pub site: Option<String>,
    pub candidates: Vec<Candidate>,
    /// Sites that were tried and gave nothing, and why.
    pub skipped: Vec<Skip>,
    pub stopped: Option<Stop>,
    /// What AO3's recent works said about the fandom, when the names came from AO3: relationships
    /// and additional tags for the tag suggestions, and AO3's own name for the fandom.
    pub profile: Option<ao3::Profile>,
}

enum Abort {
    Skip(SkipReason),
    Stop(Stop),
}

struct Run<'a> {
    fetcher: &'a mut dyn Fetcher,
    /// Set when the run was cut short after some names were already found; they are kept.
    stopped: Option<Stop>,
    requests: usize,
    max: usize,
    /// The pause before a request, which depends on the site.
    gap: Duration,
    robots: HashMap<String, Robots>,
}

impl Run<'_> {
    fn get(&mut self, url: &str) -> Result<Response, Abort> {
        if self.fetcher.cancelled() {
            return Err(Abort::Stop(Stop::Cancelled));
        }
        if self.requests >= self.max {
            return Err(Abort::Stop(Stop::OutOfRequests));
        }
        if self.requests > 0 {
            self.fetcher.pause(self.gap);
            if self.fetcher.cancelled() {
                return Err(Abort::Stop(Stop::Cancelled));
            }
        }
        self.requests += 1;
        match self.fetcher.get(url) {
            Ok(response) => Ok(response),
            Err(error) => Err(Abort::Skip(SkipReason::Network(error.to_string()))),
        }
    }

    /// `robots.txt` of `host`, fetched once per run.
    fn robots(&mut self, host: &str) -> Result<Robots, Abort> {
        if let Some(known) = self.robots.get(host) {
            return Ok(known.clone());
        }
        let robots = match self.get(&format!("https://{host}/robots.txt")) {
            Ok(response) => Robots::from_response(response.status, &response.body),
            Err(Abort::Stop(stop)) => return Err(Abort::Stop(stop)),
            // No answer at all: no telling what the site wants.
            Err(Abort::Skip(_)) => Robots::disallow_all(),
        };
        self.robots.insert(host.to_owned(), robots.clone());
        Ok(robots)
    }
}

fn host_and_path(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    Some(match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, "/"),
    })
}

/// Runs the import. Never makes more than `request.max_requests` requests, one at a time.
pub fn run(fetcher: &mut dyn Fetcher, request: &Request) -> Outcome {
    let mut run = Run {
        fetcher,
        stopped: None,
        requests: 0,
        max: request.max_requests,
        gap: REQUEST_GAP,
        robots: HashMap::new(),
    };
    let mut outcome = Outcome::default();
    for site in &request.sites {
        run.gap = REQUEST_GAP;
        if site.kind == SiteKind::Ao3 {
            run.gap = ao3::GAP;
            match try_ao3(&mut run, request, &site.name) {
                Ok((candidates, profile)) => {
                    outcome.site = Some(site.name.clone());
                    outcome.candidates = candidates;
                    outcome.profile = Some(profile);
                    outcome.stopped = run.stopped;
                    return outcome;
                }
                Err(Abort::Skip(reason)) => outcome.skipped.push(Skip {
                    site: site.name.clone(),
                    reason,
                }),
                Err(Abort::Stop(stop)) => {
                    outcome.stopped = Some(stop);
                    return outcome;
                }
            }
            continue;
        }
        if site.needs_slug() && request.slug.is_empty() {
            outcome.skipped.push(Skip {
                site: site.name.clone(),
                reason: SkipReason::NoWikiName,
            });
            continue;
        }
        for api in site.api_urls(&request.slug) {
            match try_api(&mut run, &api, request, &site.name) {
                Ok(candidates) => {
                    outcome.site = Some(site.name.clone());
                    outcome.candidates = candidates;
                    outcome.stopped = run.stopped;
                    return outcome;
                }
                Err(Abort::Skip(reason)) => {
                    let missing = reason == SkipReason::NotFound;
                    outcome.skipped.push(Skip {
                        site: site.name.clone(),
                        reason,
                    });
                    // A site whose first address was not found may keep its API elsewhere.
                    if !missing {
                        break;
                    }
                }
                Err(Abort::Stop(stop)) => {
                    outcome.stopped = Some(stop);
                    return outcome;
                }
            }
        }
    }
    outcome
}

/// AO3: the fandom's most recent works, newest first, without crossovers.
fn try_ao3(
    run: &mut Run<'_>,
    request: &Request,
    site: &str,
) -> Result<(Vec<Candidate>, ao3::Profile), Abort> {
    let robots = run.robots(ao3::HOST)?;
    let mut works: Vec<ao3::Work> = Vec::new();
    for page in 1..=ao3::MAX_PAGES {
        let url = ao3::works_url(&request.fandom, page);
        let Some((_, path)) = host_and_path(&url) else {
            return Err(Abort::Skip(SkipReason::UnexpectedPage));
        };
        if !robots.allows(ROBOTS_TOKEN, path) {
            return Err(Abort::Skip(SkipReason::RobotsDisallow));
        }
        let response = match run.get(&url) {
            Ok(response) => response,
            Err(Abort::Stop(stop)) if !works.is_empty() => {
                run.stopped = Some(stop);
                break;
            }
            Err(other) => return Err(other),
        };
        match response.status {
            429 | 503 if works.is_empty() => return Err(Abort::Skip(SkipReason::Blocked)),
            429 | 503 => {
                run.stopped = Some(Stop::SlowDown);
                break;
            }
            404 | 410 => return Err(Abort::Skip(SkipReason::NotFound)),
            200..=299 => {}
            other => return Err(Abort::Skip(SkipReason::Network(format!("HTTP {other}")))),
        }
        let listed = ao3::parse_works(&response.body);
        if listed.is_empty() {
            if works.is_empty() {
                return Err(Abort::Skip(SkipReason::UnexpectedPage));
            }
            break;
        }
        let on_page = listed.len();
        works.extend(listed);
        let kept = works.iter().filter(|work| work.fandoms.len() == 1).count();
        if kept >= ao3::MAX_WORKS || on_page < 20 {
            break;
        }
    }
    let Some(profile) = ao3::profile(&works) else {
        return Err(Abort::Skip(SkipReason::NoCharacters));
    };
    if profile.characters.is_empty() {
        return Err(Abort::Skip(SkipReason::NoCharacters));
    }
    let candidates = profile
        .characters
        .iter()
        .map(|tag| Candidate {
            name: tag.name.clone(),
            fandom: request.fandom.clone(),
            site: site.to_owned(),
            title: tag.name.clone(),
            uses: tag.uses,
        })
        .collect();
    Ok((candidates, profile))
}

fn try_api(
    run: &mut Run<'_>,
    api: &str,
    request: &Request,
    site: &str,
) -> Result<Vec<Candidate>, Abort> {
    let Some((host, _)) = host_and_path(api) else {
        return Err(Abort::Skip(SkipReason::NotMediaWiki));
    };
    let robots = run.robots(host)?;
    for category in &request.categories {
        let titles = list_category(run, &robots, api, category)?;
        let cut_short = run.stopped;
        let mut seen = HashSet::new();
        let mut found = Vec::new();
        for title in titles {
            if found.len() >= MAX_CANDIDATES {
                break;
            }
            if let Some(name) = ao3_name(&title, &request.fandom)
                && seen.insert(name.to_lowercase())
            {
                found.push(Candidate {
                    name,
                    fandom: request.fandom.clone(),
                    site: site.to_owned(),
                    title,
                    uses: 0,
                });
            }
        }
        if !found.is_empty() {
            return Ok(found);
        }
        match cut_short {
            Some(Stop::SlowDown) => return Err(Abort::Skip(SkipReason::Blocked)),
            Some(stop) => return Err(Abort::Stop(stop)),
            None => {}
        }
    }
    Err(Abort::Skip(SkipReason::NoCharacters))
}

/// The page titles in a category and in its first subcategories.
fn list_category(
    run: &mut Run<'_>,
    robots: &Robots,
    api: &str,
    category: &str,
) -> Result<Vec<String>, Abort> {
    let mut titles = Vec::new();
    let mut subcategories = Vec::new();
    let root = read_category(
        run,
        robots,
        api,
        category,
        &mut titles,
        Some(&mut subcategories),
    );
    match root {
        Ok(()) => {}
        // Cut short with names already read: keep them.
        Err(Abort::Stop(stop)) if !titles.is_empty() => {
            run.stopped = Some(stop);
            return Ok(titles);
        }
        // Held back before anything was read: this site is out, the next one may still answer.
        Err(Abort::Stop(Stop::SlowDown)) => return Err(Abort::Skip(SkipReason::Blocked)),
        Err(other) => return Err(other),
    }
    for sub in subcategories.into_iter().take(MAX_SUBCATEGORIES) {
        match read_category(run, robots, api, &sub, &mut titles, None) {
            Ok(()) => {}
            Err(Abort::Stop(stop)) => {
                // Out of requests, cancelled or asked to slow down: what was read stays.
                run.stopped = Some(stop);
                break;
            }
            // One subcategory that cannot be read does not spoil the rest.
            Err(Abort::Skip(_)) => {}
        }
    }
    Ok(titles)
}

fn read_category(
    run: &mut Run<'_>,
    robots: &Robots,
    api: &str,
    category: &str,
    titles: &mut Vec<String>,
    mut subcategories: Option<&mut Vec<String>>,
) -> Result<(), Abort> {
    let mut next: Option<String> = None;
    for _ in 0..MAX_PAGES_PER_CATEGORY {
        let url = mediawiki::members_url(api, category, next.as_deref());
        let Some((_, path)) = host_and_path(&url) else {
            return Err(Abort::Skip(SkipReason::NotMediaWiki));
        };
        if !robots.allows(ROBOTS_TOKEN, path) {
            return Err(Abort::Skip(SkipReason::RobotsDisallow));
        }
        let response = run.get(&url)?;
        match response.status {
            429 | 503 => return Err(Abort::Stop(Stop::SlowDown)),
            404 | 410 => return Err(Abort::Skip(SkipReason::NotFound)),
            200..=299 => {}
            other => return Err(Abort::Skip(SkipReason::Network(format!("HTTP {other}")))),
        }
        let members = match mediawiki::parse_members(&response.body) {
            Ok(members) => members,
            Err(ApiError::NotMediaWiki) => return Err(Abort::Skip(SkipReason::NotMediaWiki)),
            Err(ApiError::Refused(code)) if code == "maxlag" => {
                return Err(Abort::Stop(Stop::SlowDown));
            }
            Err(ApiError::Refused(code)) => return Err(Abort::Skip(SkipReason::Refused(code))),
        };
        titles.extend(members.pages);
        if let Some(subs) = subcategories.as_deref_mut() {
            subs.extend(members.subcategories);
        }
        match members.next {
            Some(token) => next = Some(token),
            None => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sites::built_in;

    /// Answers by address (the longest matching prefix) and keeps a record.
    #[derive(Default)]
    struct Fake {
        answers: Vec<(String, Result<Response, FetchError>)>,
        asked: Vec<String>,
        pauses: usize,
        cancel_after: Option<usize>,
    }

    impl Fake {
        fn on(mut self, prefix: &str, status: u16, body: &str) -> Self {
            self.answers.push((
                prefix.to_owned(),
                Ok(Response {
                    status,
                    body: body.to_owned(),
                }),
            ));
            self
        }
    }

    impl Fetcher for Fake {
        fn get(&mut self, url: &str) -> Result<Response, FetchError> {
            self.asked.push(url.to_owned());
            self.answers
                .iter()
                .filter(|(prefix, _)| url.starts_with(prefix.as_str()))
                .max_by_key(|(prefix, _)| prefix.len())
                .map(|(_, answer)| answer.clone())
                .unwrap_or(Err(FetchError::Network("no route".into())))
        }
        fn pause(&mut self, _: Duration) {
            self.pauses += 1;
        }
        fn cancelled(&self) -> bool {
            self.cancel_after.is_some_and(|n| self.asked.len() >= n)
        }
    }

    fn listing(pages: &[&str], subs: &[&str]) -> String {
        let mut items: Vec<String> = pages
            .iter()
            .map(|t| format!(r#"{{"ns":0,"title":"{t}"}}"#))
            .collect();
        items.extend(
            subs.iter()
                .map(|s| format!(r#"{{"ns":14,"title":"Category:{s}"}}"#)),
        );
        format!(r#"{{"query":{{"categorymembers":[{}]}}}}"#, items.join(","))
    }

    fn request(sites: &[&str]) -> Request {
        Request {
            fandom: "Harry Potter".into(),
            slug: "harrypotter".into(),
            sites: built_in()
                .into_iter()
                .filter(|s| sites.contains(&s.id.as_str()))
                .collect(),
            categories: vec!["Characters".into()],
            max_requests: 40,
        }
    }

    const FANDOM_API: &str = "https://harrypotter.fandom.com/api.php";
    const MIRAHEZE_API: &str = "https://harrypotter.miraheze.org/w/api.php";

    #[test]
    fn the_first_site_with_characters_wins_and_names_come_back_in_ao3_form() {
        let mut fake = Fake::default()
            .on("https://harrypotter.fandom.com/robots.txt", 404, "")
            .on(&format!("{FANDOM_API}?action=query&list=categorymembers&cmtitle=Category%3ACharacters"),
                200, &listing(&["Granger, Hermione", "Harry Potter", "List of characters", "Rey (character)"], &["Hogwarts students"]))
            .on(&format!("{FANDOM_API}?action=query&list=categorymembers&cmtitle=Category%3AHogwarts_students"),
                200, &listing(&["Ron Weasley", "Harry Potter"], &[]));
        let outcome = run(&mut fake, &request(&["fandom", "miraheze"]));
        assert_eq!(outcome.site.as_deref(), Some("Fandom"));
        let names: Vec<&str> = outcome.candidates.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["Hermione Granger", "Harry Potter", "Rey", "Ron Weasley"]
        );
        assert!(
            outcome
                .candidates
                .iter()
                .all(|c| c.fandom == "Harry Potter")
        );
        assert!(
            fake.asked.iter().all(|u| !u.contains("miraheze")),
            "the second site is never asked"
        );
        assert_eq!(
            fake.pauses,
            fake.asked.len() - 1,
            "a pause before every request but the first"
        );
    }

    #[test]
    fn a_site_that_disallows_the_api_is_skipped_and_the_next_one_is_tried() {
        let mut fake = Fake::default()
            .on(
                "https://harrypotter.fandom.com/robots.txt",
                200,
                "User-agent: *\nDisallow: /api.php\n",
            )
            .on("https://harrypotter.miraheze.org/robots.txt", 404, "")
            .on(MIRAHEZE_API, 200, &listing(&["Ron Weasley"], &[]));
        let outcome = run(&mut fake, &request(&["fandom", "miraheze"]));
        assert_eq!(outcome.site.as_deref(), Some("Miraheze"));
        assert_eq!(
            outcome.skipped,
            [Skip {
                site: "Fandom".into(),
                reason: SkipReason::RobotsDisallow
            }]
        );
        assert!(
            fake.asked.iter().all(|u| !u.starts_with(FANDOM_API)),
            "nothing sent to a disallowed path"
        );
    }

    #[test]
    fn a_robots_file_that_cannot_be_read_means_nothing_is_asked() {
        let mut fake = Fake::default().on("https://harrypotter.fandom.com/robots.txt", 500, "");
        let outcome = run(&mut fake, &request(&["fandom"]));
        assert!(outcome.candidates.is_empty());
        assert_eq!(fake.asked, ["https://harrypotter.fandom.com/robots.txt"]);

        // No answer at all is the same.
        let mut silent = Fake::default();
        let outcome = run(&mut silent, &request(&["fandom"]));
        assert!(outcome.candidates.is_empty());
        assert_eq!(silent.asked.len(), 1);
    }

    #[test]
    fn a_challenge_page_a_missing_wiki_and_an_empty_category_are_reported_and_skipped() {
        let mut fake = Fake::default()
            .on("https://harrypotter.fandom.com/robots.txt", 404, "")
            .on(FANDOM_API, 200, "<html>Just a moment...</html>")
            .on("https://harrypotter.miraheze.org/robots.txt", 404, "")
            .on(MIRAHEZE_API, 404, "")
            .on("https://harrypotter.wiki.gg/robots.txt", 404, "")
            .on(
                "https://harrypotter.wiki.gg/api.php",
                200,
                &listing(&[], &[]),
            );
        let outcome = run(&mut fake, &request(&["fandom", "miraheze", "wiki.gg"]));
        let reasons: Vec<&SkipReason> = outcome.skipped.iter().map(|s| &s.reason).collect();
        assert_eq!(
            reasons,
            [
                &SkipReason::NotMediaWiki,
                &SkipReason::NotFound,
                &SkipReason::NoCharacters
            ]
        );
        assert!(outcome.candidates.is_empty() && outcome.stopped.is_none());
    }

    #[test]
    fn a_site_asking_for_slower_requests_is_left_alone_and_the_next_site_is_tried() {
        for (status, body) in [
            (429, ""),
            (503, ""),
            (200, r#"{"error":{"code":"maxlag"}}"#),
        ] {
            let mut fake = Fake::default()
                .on("https://harrypotter.fandom.com/robots.txt", 404, "")
                .on(FANDOM_API, status, body)
                .on("https://harrypotter.miraheze.org/robots.txt", 404, "")
                .on(MIRAHEZE_API, 200, &listing(&["Ron Weasley"], &[]));
            let outcome = run(&mut fake, &request(&["fandom", "miraheze"]));
            assert_eq!(
                outcome.skipped[0].reason,
                SkipReason::Blocked,
                "{status} {body}"
            );
            assert_eq!(outcome.site.as_deref(), Some("Miraheze"));
            let to_fandom = fake
                .asked
                .iter()
                .filter(|u| u.contains("fandom.com"))
                .count();
            assert_eq!(
                to_fandom, 2,
                "the blocked site is asked once after its robots file: {status}"
            );
        }
    }

    // ------------------------------------------------------------------ AO3

    use crate::ao3::tests::work_html;

    fn ao3_page(first: u32, count: u32, crossover_every: u32) -> String {
        let works: String = (first..first + count)
            .map(|id| {
                let fandoms: &[&str] = if crossover_every > 0 && id % crossover_every == 0 {
                    &["Hades (Video Game)", "Other"]
                } else {
                    &["Hades (Video Game)"]
                };
                work_html(
                    id,
                    fandoms,
                    &["Zagreus/Megaera"],
                    &["Zagreus (Hades)", &format!("Extra {id}")],
                    &["Fluff"],
                )
            })
            .collect();
        format!("<ol class=\"work index group\">{works}</ol>")
    }

    const AO3_ROBOTS: &str =
        "User-agent: *\nDisallow: /works?\nDisallow: /autocomplete/\nDisallow: /tags/search?\n";

    fn ao3_request() -> Request {
        let mut r = request(&["ao3", "fandom"]);
        r.fandom = "Hades (Video Game)".into();
        r.slug = "hades".into();
        r
    }

    #[test]
    fn ao3_comes_first_reads_recent_works_without_crossovers_and_stops_at_fifty() {
        let mut fake = Fake::default()
            .on("https://archiveofourown.org/robots.txt", 200, AO3_ROBOTS)
            .on(
                "https://archiveofourown.org/tags/Hades%20%28Video%20Game%29/works?page=1",
                200,
                &ao3_page(1, 20, 6),
            )
            .on(
                "https://archiveofourown.org/tags/Hades%20%28Video%20Game%29/works?page=2",
                200,
                &ao3_page(21, 20, 6),
            )
            .on(
                "https://archiveofourown.org/tags/Hades%20%28Video%20Game%29/works?page=3",
                200,
                &ao3_page(41, 20, 6),
            );
        let outcome = run(&mut fake, &ao3_request());
        assert_eq!(outcome.site.as_deref(), Some("AO3"));
        let profile = outcome.profile.expect("a profile");
        assert_eq!(profile.works, 50, "fifty works that are not crossovers");
        assert_eq!(profile.fandom, "Hades (Video Game)");
        assert_eq!(profile.relationships[0].name, "Zagreus/Megaera");
        assert_eq!(outcome.candidates[0].name, "Zagreus (Hades)");
        assert_eq!(outcome.candidates[0].uses, 50);
        assert!(
            outcome
                .candidates
                .iter()
                .all(|c| c.fandom == "Hades (Video Game)" && c.site == "AO3")
        );
        // robots.txt plus three pages (40 works hold 34 non-crossovers, so a third page is read).
        assert_eq!(fake.asked.len(), 4);
        assert!(
            fake.asked
                .iter()
                .all(|u| u.starts_with("https://archiveofourown.org/")),
            "the wiki list is not touched"
        );
        assert!(
            fake.asked.iter().skip(1).all(|u| u.contains("/tags/")),
            "only the fandom's work list"
        );
        assert_eq!(fake.pauses, 3);
    }

    #[test]
    fn when_ao3_holds_the_client_back_the_wiki_list_is_used() {
        let mut fake = Fake::default()
            .on("https://archiveofourown.org/robots.txt", 200, AO3_ROBOTS)
            .on(
                "https://archiveofourown.org/tags/",
                429,
                "The bot servers are currently overloaded",
            )
            .on("https://hades.fandom.com/robots.txt", 404, "")
            .on(
                "https://hades.fandom.com/api.php",
                200,
                &listing(&["Zagreus"], &[]),
            );
        let outcome = run(&mut fake, &ao3_request());
        assert_eq!(
            outcome.skipped,
            [Skip {
                site: "AO3".into(),
                reason: SkipReason::Blocked
            }]
        );
        assert_eq!(outcome.site.as_deref(), Some("Fandom"));
        assert!(outcome.profile.is_none());
        assert_eq!(
            fake.asked
                .iter()
                .filter(|u| u.contains("archiveofourown"))
                .count(),
            2,
            "robots.txt and one page, no more"
        );
    }

    #[test]
    fn ao3_falls_back_when_robots_says_no_the_page_is_unexpected_or_the_tag_is_missing() {
        for (robots, page, reason) in [
            (
                "User-agent: *\nDisallow: /tags/\n",
                (200, "<html></html>"),
                SkipReason::RobotsDisallow,
            ),
            (
                AO3_ROBOTS,
                (200, "<html>Please wait...</html>"),
                SkipReason::UnexpectedPage,
            ),
            (AO3_ROBOTS, (404, "nope"), SkipReason::NotFound),
        ] {
            let mut fake = Fake::default()
                .on("https://archiveofourown.org/robots.txt", 200, robots)
                .on("https://archiveofourown.org/tags/", page.0, page.1)
                .on("https://hades.fandom.com/robots.txt", 404, "")
                .on(
                    "https://hades.fandom.com/api.php",
                    200,
                    &listing(&["Zagreus"], &[]),
                );
            let outcome = run(&mut fake, &ao3_request());
            assert_eq!(outcome.skipped[0].reason, reason);
            assert_eq!(outcome.site.as_deref(), Some("Fandom"));
        }
        // An unreadable robots.txt means AO3 is not asked at all.
        let mut fake = Fake::default()
            .on("https://archiveofourown.org/robots.txt", 503, "")
            .on("https://hades.fandom.com/robots.txt", 404, "")
            .on(
                "https://hades.fandom.com/api.php",
                200,
                &listing(&["Zagreus"], &[]),
            );
        let outcome = run(&mut fake, &ao3_request());
        assert!(fake.asked.iter().all(|u| !u.contains("/tags/")));
        assert_eq!(outcome.site.as_deref(), Some("Fandom"));
    }

    #[test]
    fn a_block_part_way_keeps_the_works_already_read() {
        let mut fake = Fake::default()
            .on("https://archiveofourown.org/robots.txt", 200, AO3_ROBOTS)
            .on(
                "https://archiveofourown.org/tags/Hades%20%28Video%20Game%29/works?page=1",
                200,
                &ao3_page(1, 20, 0),
            )
            .on(
                "https://archiveofourown.org/tags/Hades%20%28Video%20Game%29/works?page=2",
                429,
                "",
            );
        let outcome = run(&mut fake, &ao3_request());
        assert_eq!(outcome.site.as_deref(), Some("AO3"));
        assert_eq!(outcome.profile.unwrap().works, 20);
        assert_eq!(outcome.stopped, Some(Stop::SlowDown));
    }

    #[test]
    fn the_request_budget_and_cancel_are_honoured() {
        let mut fake = Fake::default()
            .on("https://harrypotter.fandom.com/robots.txt", 404, "")
            .on(FANDOM_API, 200, &listing(&["A One"], &["S1", "S2", "S3"]));
        let mut limited = request(&["fandom"]);
        limited.max_requests = 3;
        let outcome = run(&mut fake, &limited);
        assert_eq!(outcome.stopped, Some(Stop::OutOfRequests));
        assert_eq!(fake.asked.len(), 3);
        assert_eq!(
            outcome.candidates.len(),
            1,
            "the names read before the limit are kept"
        );

        let mut fake = Fake::default()
            .on("https://harrypotter.fandom.com/robots.txt", 404, "")
            .on(FANDOM_API, 200, &listing(&["A One"], &[]));
        fake.cancel_after = Some(1);
        let outcome = run(&mut fake, &request(&["fandom"]));
        assert_eq!(outcome.stopped, Some(Stop::Cancelled));
        assert_eq!(fake.asked.len(), 1, "only the robots file went out");
    }

    #[test]
    fn the_next_category_is_tried_when_the_first_is_empty() {
        let mut fake = Fake::default()
            .on("https://en.wikipedia.org/robots.txt", 200, "User-agent: *\nDisallow: /w/\nAllow: /w/api.php\n")
            .on("https://en.wikipedia.org/w/api.php?action=query&list=categorymembers&cmtitle=Category%3ACharacters", 200, &listing(&[], &[]))
            .on("https://en.wikipedia.org/w/api.php?action=query&list=categorymembers&cmtitle=Category%3AHarry_Potter_characters", 200, &listing(&["Hermione Granger"], &[]));
        let mut asked = request(&["wikipedia"]);
        asked.categories = vec!["Characters".into(), "Harry Potter characters".into()];
        let outcome = run(&mut fake, &asked);
        assert_eq!(outcome.site.as_deref(), Some("Wikipedia"));
        assert_eq!(outcome.candidates.len(), 1);
    }

    #[test]
    fn a_fandom_with_no_wiki_name_only_tries_sites_that_do_not_need_one() {
        let mut fake = Fake::default()
            .on("https://en.wikipedia.org/robots.txt", 404, "")
            .on(
                "https://en.wikipedia.org/w/api.php",
                200,
                &listing(&["Rey"], &[]),
            );
        let mut asked = request(&["fandom", "wikipedia"]);
        asked.slug.clear();
        let outcome = run(&mut fake, &asked);
        assert_eq!(outcome.skipped[0].reason, SkipReason::NoWikiName);
        assert_eq!(outcome.site.as_deref(), Some("Wikipedia"));
        assert!(USER_AGENT.contains("bot"));
    }
}
