//! `libertum-wiki`: character names from MediaWiki sites (Fandom, Miraheze, Wikipedia and the
//! like), for the Characters tab's Import. No UI and no HTTP client of its own: the caller gives
//! the importer a [`Fetcher`], so the whole flow is tested without a network.
//!
//! The rules it keeps, because the writer agrees to online imports at their own risk and only
//! on a manual action:
//!
//! - It reads a site's `robots.txt` first and does not touch a path the file disallows
//!   ([`robots`], RFC 9309: a missing file allows everything; a server error or no answer allows
//!   nothing).
//! - It uses the MediaWiki Action API (`api.php`), never page scraping, one request at a time with
//!   a pause between them, a cap on how many it makes, and a User-Agent that says it is a bot.
//! - It stops at once when a site asks it to slow down.
//! - Everything it brings back is named the way AO3 writes character tags ([`names`]).

pub mod ao3;
pub mod import;
pub mod mediawiki;
pub mod names;
pub mod robots;
pub mod sites;

pub use ao3::{Profile, Tagged};
pub use import::{
    Candidate, FetchError, Fetcher, Outcome, REQUEST_GAP, ROBOTS_TOKEN, Request, Response, Skip,
    SkipReason, Stop, USER_AGENT, run,
};
pub use names::ao3_name;
pub use robots::Robots;
pub use sites::{Site, SiteError, SiteKind, built_in, parse_custom, slug};
