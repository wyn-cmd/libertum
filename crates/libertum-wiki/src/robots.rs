//! `robots.txt`, read the way RFC 9309 says.

/// What a site's `robots.txt` allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Robots {
    kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    AllowAll,
    DisallowAll,
    Groups(Vec<Group>),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct Group {
    agents: Vec<String>,
    /// (allow, pattern), in file order.
    rules: Vec<(bool, String)>,
}

impl Robots {
    pub fn allow_all() -> Self {
        Self {
            kind: Kind::AllowAll,
        }
    }

    pub fn disallow_all() -> Self {
        Self {
            kind: Kind::DisallowAll,
        }
    }

    /// What the answer to a request for `/robots.txt` means. A success is read; any other
    /// 4xx (the file does not exist, or is not for us to see) allows everything; a server error
    /// or anything else allows nothing, because there is no telling what the site wants.
    pub fn from_response(status: u16, body: &str) -> Self {
        match status {
            200..=299 => Self::parse(body),
            400..=499 => Self::allow_all(),
            _ => Self::disallow_all(),
        }
    }

    pub fn parse(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut groups: Vec<Group> = Vec::new();
        let mut current: Option<Group> = None;
        let mut rules_started = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "user-agent" => {
                    if rules_started || current.is_none() {
                        if let Some(done) = current.take() {
                            groups.push(done);
                        }
                        current = Some(Group::default());
                        rules_started = false;
                    }
                    if let Some(group) = current.as_mut() {
                        group.agents.push(value.to_ascii_lowercase());
                    }
                }
                "allow" | "disallow" => {
                    rules_started = true;
                    if let Some(group) = current.as_mut() {
                        // An empty Disallow allows everything: no rule at all.
                        if !value.is_empty() {
                            group.rules.push((key == "allow", value.to_owned()));
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(done) = current {
            groups.push(done);
        }
        Self {
            kind: Kind::Groups(groups),
        }
    }

    /// Whether `agent` (the product token, such as `libertum`) may fetch `path`, which is the
    /// path and query as sent (`/w/api.php?action=query`).
    pub fn allows(&self, agent: &str, path: &str) -> bool {
        let groups = match &self.kind {
            Kind::AllowAll => return true,
            Kind::DisallowAll => return false,
            Kind::Groups(groups) => groups,
        };
        let agent = agent.to_ascii_lowercase();
        let named: Vec<&Group> = groups
            .iter()
            .filter(|group| group.agents.contains(&agent))
            .collect();
        let chosen: Vec<&Group> = if named.is_empty() {
            groups
                .iter()
                .filter(|group| group.agents.iter().any(|a| a == "*"))
                .collect()
        } else {
            named
        };
        // The longest matching pattern wins; Allow wins a tie.
        let mut best: Option<(usize, bool)> = None;
        for group in chosen {
            for (allow, pattern) in &group.rules {
                if matches(pattern, path) {
                    let length = pattern.len();
                    best = match best {
                        Some((len, was)) if len > length || (len == length && was) => {
                            Some((len, was))
                        }
                        _ => Some((length, *allow)),
                    };
                }
            }
        }
        best.is_none_or(|(_, allow)| allow)
    }
}

/// A robots pattern against a path: a prefix match, where `*` matches anything and a final `$`
/// pins the end.
fn matches(pattern: &str, path: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(rest) => (rest, true),
        None => (pattern, false),
    };
    let parts: Vec<&str> = pattern.split('*').collect();
    if !path.starts_with(parts[0]) {
        return false;
    }
    let mut position = parts[0].len();
    for (index, part) in parts.iter().enumerate().skip(1) {
        let last = index == parts.len() - 1;
        if last && anchored {
            return path.len() >= position + part.len() && path.ends_with(part);
        }
        match path[position..].find(part) {
            Some(found) => position += found + part.len(),
            None => return false,
        }
    }
    !anchored || position == path.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: &str = "libertum";

    #[test]
    fn a_missing_file_allows_everything_and_a_broken_server_allows_nothing() {
        assert!(Robots::from_response(404, "").allows(AGENT, "/api.php"));
        assert!(Robots::from_response(403, "<html>Just a moment").allows(AGENT, "/api.php"));
        assert!(!Robots::from_response(500, "").allows(AGENT, "/api.php"));
        assert!(!Robots::from_response(503, "").allows(AGENT, "/api.php"));
        assert!(Robots::from_response(200, "").allows(AGENT, "/anything"));
    }

    #[test]
    fn the_wildcard_group_applies_unless_the_bot_is_named() {
        let text =
            "User-agent: *\nDisallow: /w/\nAllow: /w/api.php\n\nUser-agent: Badbot\nDisallow: /\n";
        let robots = Robots::parse(text);
        assert!(
            robots.allows(AGENT, "/w/api.php?action=query"),
            "longer Allow beats Disallow"
        );
        assert!(!robots.allows(AGENT, "/w/index.php"));
        assert!(robots.allows(AGENT, "/wiki/Page"));
        assert!(!robots.allows("badbot", "/wiki/Page"));

        let named = Robots::parse("User-agent: *\nDisallow: /\n\nUser-agent: Libertum\nAllow: /\n");
        assert!(
            named.allows(AGENT, "/api.php"),
            "a group that names us replaces the wildcard"
        );
    }

    #[test]
    fn wildcards_anchors_ties_and_empty_disallow_work() {
        let robots = Robots::parse(
            "User-agent: *\nDisallow: /*.php$\nDisallow: /private\nAllow: /private/open\n",
        );
        assert!(!robots.allows(AGENT, "/api.php"));
        assert!(
            robots.allows(AGENT, "/api.php?x=1"),
            "the anchor needs the path to end there"
        );
        assert!(!robots.allows(AGENT, "/private/x"));
        assert!(robots.allows(AGENT, "/private/open/y"));
        let tie = Robots::parse("User-agent: *\nDisallow: /a\nAllow: /a\n");
        assert!(tie.allows(AGENT, "/a"), "Allow wins a tie");
        assert!(Robots::parse("User-agent: *\nDisallow:\n").allows(AGENT, "/x"));
    }

    #[test]
    fn several_agents_share_a_group_comments_and_case_are_ignored() {
        let text = "\u{feff}# hi\nUSER-AGENT: a\nuser-agent: Libertum # us\nDISALLOW: /x # no\n";
        let robots = Robots::parse(text);
        assert!(!robots.allows(AGENT, "/x/1"));
        assert!(robots.allows(AGENT, "/y"));
        assert!(
            Robots::parse("Disallow: /\n").allows(AGENT, "/x"),
            "a rule before any agent is ignored"
        );
    }
}
