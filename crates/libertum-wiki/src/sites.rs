//! The MediaWiki sites an import can look in, most useful first.
//!
//! Chosen by size and by how many fandom wikis they host (ArchiveTeam's wiki-farm tables, 2026:
//! Fandom about 261,000 wikis, Miraheze the next largest, with WikiTide merged into it in April
//! 2024, ShoutWiki about 12,000; wiki.gg hosts game wikis; Wikipedia covers the large franchises).
//! Fandom is first, as asked. Each is reached through its `api.php`; the paths below are the
//! usual ones, and a site that answers differently is simply skipped.

use serde::{Deserialize, Serialize};

/// What kind of site this is, which decides where its API lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteKind {
    /// Archive of Our Own, read through its fandom tag's work list.
    Ao3,
    Fandom,
    Miraheze,
    WikiGg,
    ShoutWiki,
    Wikipedia,
    /// An address the writer added.
    Custom,
}

/// One place to look for characters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    /// Stable across runs and versions: what the settings refer to.
    pub id: String,
    pub name: String,
    pub kind: SiteKind,
    /// The base address of a custom site (`https://wiki.example.org`), else empty.
    #[serde(default)]
    pub base: String,
}

impl Site {
    /// The API addresses to try for a fandom wiki called `slug`, in order.
    pub fn api_urls(&self, slug: &str) -> Vec<String> {
        match self.kind {
            // AO3 has no API; its pages are addressed by `ao3::works_url`.
            SiteKind::Ao3 => Vec::new(),
            SiteKind::Fandom => vec![format!("https://{slug}.fandom.com/api.php")],
            SiteKind::Miraheze => vec![format!("https://{slug}.miraheze.org/w/api.php")],
            SiteKind::ShoutWiki => vec![format!("https://{slug}.shoutwiki.com/w/api.php")],
            SiteKind::WikiGg => vec![format!("https://{slug}.wiki.gg/api.php")],
            SiteKind::Wikipedia => vec!["https://en.wikipedia.org/w/api.php".to_owned()],
            SiteKind::Custom => vec![
                format!("{}/api.php", self.base),
                format!("{}/w/api.php", self.base),
            ],
        }
    }

    /// Whether the address depends on the fandom's wiki name.
    pub fn needs_slug(&self) -> bool {
        !matches!(
            self.kind,
            SiteKind::Wikipedia | SiteKind::Custom | SiteKind::Ao3
        )
    }
}

/// The sites that come with Libertum, in the order they are tried.
pub fn built_in() -> Vec<Site> {
    let site = |id: &str, name: &str, kind| Site {
        id: id.to_owned(),
        name: name.to_owned(),
        kind,
        base: String::new(),
    };
    vec![
        site("ao3", "AO3", SiteKind::Ao3),
        site("fandom", "Fandom", SiteKind::Fandom),
        site("miraheze", "Miraheze", SiteKind::Miraheze),
        site("wiki.gg", "wiki.gg", SiteKind::WikiGg),
        site("shoutwiki", "ShoutWiki", SiteKind::ShoutWiki),
        site("wikipedia", "Wikipedia", SiteKind::Wikipedia),
    ]
}

/// A wiki's subdomain for a fandom: lower-case ASCII letters and digits only (`Harry Potter`
/// is `harrypotter`). `None` when nothing is left (a title in another script).
pub fn slug(fandom: &str) -> Option<String> {
    let slug: String = fandom
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (!slug.is_empty() && slug.len() <= 60).then_some(slug)
}

/// Why an address cannot be added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SiteError {
    #[error("the address must start with https://")]
    NotHttps,
    #[error("that does not look like a website address")]
    BadAddress,
    #[error("that address points at this computer or a private network")]
    Private,
}

/// A custom site from the address the writer typed. Only `https` addresses are taken, with
/// no login, port or query; one that names this computer or a private network is refused. A
/// trailing `api.php` or `/w` is cut off, so the wiki's home page address works too.
pub fn parse_custom(input: &str) -> Result<Site, SiteError> {
    let input = input.trim();
    let rest = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("HTTPS://"))
        .ok_or(SiteError::NotHttps)?;
    if rest.contains(['@', '?', '#', ' ', '\\']) {
        return Err(SiteError::BadAddress);
    }
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, path),
        None => (rest, ""),
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty()
        || host.contains(':')
        || !host.contains('.')
        || host.starts_with(['.', '-'])
        || host.ends_with(['.', '-'])
        || host.contains("..")
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(SiteError::BadAddress);
    }
    let last = host.rsplit('.').next().unwrap_or("");
    if host == "localhost"
        || last.chars().all(|c| c.is_ascii_digit())
        || matches!(
            last,
            "local" | "internal" | "lan" | "home" | "localdomain" | "corp"
        )
    {
        return Err(SiteError::Private);
    }
    let mut path = path.trim_matches('/').to_owned();
    for tail in ["api.php", "index.php"] {
        if let Some(stripped) = path.strip_suffix(tail) {
            path = stripped.trim_matches('/').to_owned();
        }
    }
    if let Some(stripped) = path.strip_suffix("w")
        && (stripped.is_empty() || stripped.ends_with('/'))
    {
        path = stripped.trim_matches('/').to_owned();
    }
    if path.contains("//") {
        return Err(SiteError::BadAddress);
    }
    let base = if path.is_empty() {
        format!("https://{host}")
    } else {
        format!("https://{host}/{path}")
    };
    Ok(Site {
        id: format!("custom:{host}"),
        name: host,
        kind: SiteKind::Custom,
        base,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fandom_is_first_and_every_site_has_a_distinct_id() {
        let sites = built_in();
        assert_eq!(sites[0].kind, SiteKind::Ao3, "AO3 is the preferred source");
        assert_eq!(sites[1].kind, SiteKind::Fandom, "then Fandom");
        let mut ids: Vec<&str> = sites.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), sites.len());
        assert_eq!(
            sites[1].api_urls("harrypotter"),
            ["https://harrypotter.fandom.com/api.php"]
        );
        assert_eq!(
            sites[5].api_urls("x"),
            ["https://en.wikipedia.org/w/api.php"]
        );
        assert!(sites[0].api_urls("x").is_empty() && !sites[0].needs_slug());
    }

    #[test]
    fn a_wiki_name_comes_from_the_fandom() {
        assert_eq!(slug("Harry Potter").as_deref(), Some("harrypotter"));
        assert_eq!(
            slug("Star Wars: The Clone Wars").as_deref(),
            Some("starwarstheclonewars")
        );
        assert_eq!(slug("ハリー"), None);
        assert_eq!(slug("  "), None);
    }

    #[test]
    fn a_custom_address_is_checked_and_tidied() {
        let site = parse_custom("https://wiki.example.org/w/api.php").unwrap();
        assert_eq!(site.base, "https://wiki.example.org");
        assert_eq!(site.id, "custom:wiki.example.org");
        assert_eq!(
            site.api_urls("ignored"),
            [
                "https://wiki.example.org/api.php",
                "https://wiki.example.org/w/api.php"
            ]
        );
        assert_eq!(
            parse_custom("https://Example.ORG/wiki/").unwrap().base,
            "https://example.org/wiki"
        );
        assert_eq!(parse_custom("http://example.org"), Err(SiteError::NotHttps));
        assert_eq!(parse_custom("example.org"), Err(SiteError::NotHttps));
        for bad in [
            "https://",
            "https://nodot",
            "https://a b.org",
            "https://u@x.org",
            "https://x.org:8080",
            "https://x.org/?a=1",
        ] {
            assert_eq!(parse_custom(bad), Err(SiteError::BadAddress), "{bad}");
        }
        for private in [
            "https://localhost.local",
            "https://192.168.0.1",
            "https://printer.lan",
            "https://10.0.0.5/w",
        ] {
            assert_eq!(parse_custom(private), Err(SiteError::Private), "{private}");
        }
    }
}
