//! Shared web-tool helpers: the SSRF guard and the search scrapers.
//!
//! `fetch_url`, `web_search`, and `web.run` are thin surfaces over this
//! module so security and parsing behavior cannot drift between tools.

pub(crate) mod guard;
pub mod scrape;
