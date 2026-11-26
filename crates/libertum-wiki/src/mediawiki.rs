//! The two MediaWiki Action API calls an import makes, and reading their answers.

use serde_json::Value;

/// Percent-encodes a query value (everything but unreserved characters).
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// How many members one request asks for (the most an anonymous client may have).
pub const PAGE_SIZE: u32 = 500;

/// The address that lists a category's pages and subcategories. `category` is the name without
/// the `Category:` prefix. `maxlag` asks the server to refuse the request when it is busy,
/// which is the polite thing for a background job.
pub fn members_url(api: &str, category: &str, continue_from: Option<&str>) -> String {
    let mut url = format!(
        "{api}?action=query&list=categorymembers&cmtitle=Category%3A{}&cmlimit={PAGE_SIZE}\
         &cmtype=page%7Csubcat&cmnamespace=0%7C14&format=json&formatversion=2&maxlag=5",
        encode(&category.trim().replace(' ', "_")),
    );
    if let Some(token) = continue_from {
        url.push_str("&cmcontinue=");
        url.push_str(&encode(token));
    }
    url
}

/// One page of a category listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Members {
    /// Titles of ordinary pages.
    pub pages: Vec<String>,
    /// Names of subcategories, without the `Category:` prefix.
    pub subcategories: Vec<String>,
    /// Pass this back to get the next page.
    pub next: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    /// The answer was not the API's JSON (an HTML page, a challenge page, a wrong address).
    #[error("the site did not answer like a MediaWiki")]
    NotMediaWiki,
    /// The API reported an error; the code is its own (`maxlag`, `readapidenied`).
    #[error("the site refused: {0}")]
    Refused(String),
}

pub fn parse_members(body: &str) -> Result<Members, ApiError> {
    let value: Value = serde_json::from_str(body).map_err(|_| ApiError::NotMediaWiki)?;
    if let Some(error) = value.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        return Err(ApiError::Refused(code));
    }
    let list = value
        .pointer("/query/categorymembers")
        .and_then(Value::as_array)
        .ok_or(ApiError::NotMediaWiki)?;
    let mut members = Members::default();
    for item in list {
        let Some(title) = item.get("title").and_then(Value::as_str) else {
            continue;
        };
        match item.get("ns").and_then(Value::as_i64) {
            Some(0) => members.pages.push(title.to_owned()),
            Some(14) => members.subcategories.push(
                title
                    .split_once(':')
                    .map_or(title, |(_, name)| name)
                    .to_owned(),
            ),
            _ => {}
        }
    }
    members.next = value
        .pointer("/continue/cmcontinue")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_address_is_encoded() {
        let url = members_url(
            "https://x.fandom.com/api.php",
            "Hogwarts students",
            Some("page|AB C"),
        );
        assert!(url.starts_with("https://x.fandom.com/api.php?action=query&list=categorymembers"));
        assert!(url.contains("cmtitle=Category%3AHogwarts_students"));
        assert!(url.contains("cmcontinue=page%7CAB%20C"));
        assert!(url.contains("maxlag=5") && url.contains("formatversion=2"));
        assert_eq!(encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
    }

    #[test]
    fn members_are_split_into_pages_and_subcategories() {
        let body = r#"{"batchcomplete":true,"continue":{"cmcontinue":"page|X","continue":"-||"},
            "query":{"categorymembers":[
              {"pageid":1,"ns":0,"title":"Harry Potter"},
              {"pageid":2,"ns":14,"title":"Category:Hogwarts students"},
              {"pageid":3,"ns":6,"title":"File:A.png"}]}}"#;
        let members = parse_members(body).unwrap();
        assert_eq!(members.pages, ["Harry Potter"]);
        assert_eq!(members.subcategories, ["Hogwarts students"]);
        assert_eq!(members.next.as_deref(), Some("page|X"));
    }

    #[test]
    fn errors_and_wrong_answers_are_told_apart() {
        assert_eq!(
            parse_members("<html>Just a moment...</html>"),
            Err(ApiError::NotMediaWiki)
        );
        assert_eq!(parse_members("{}"), Err(ApiError::NotMediaWiki));
        assert_eq!(
            parse_members(r#"{"error":{"code":"maxlag","info":"busy"}}"#),
            Err(ApiError::Refused("maxlag".to_owned()))
        );
        assert_eq!(
            parse_members(r#"{"query":{"categorymembers":[]}}"#).unwrap(),
            Members::default()
        );
    }
}
