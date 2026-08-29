//! Pure keyword matcher over plugin metadata.
//!
//! Matches live as data: explicit `keywords`, `domains` (scheme / `www.` /
//! path stripped), and the plugin `name`. There is no `regex` dependency —
//! matching is substring search guarded by ASCII word boundaries.

use std::cmp::Reverse;

/// A plugin to match a draft against.
pub struct KeywordCandidate<'a> {
    pub name: &'a str,
    pub domains: &'a [String],
    pub keywords: &'a [String],
}

/// Return the index of the single candidate whose keyword matches `draft`.
///
/// Returns `None` when `draft` has fewer than 3 characters or nothing matches.
/// Longer keywords take precedence; a keyword matches only when the occurrence
/// is flanked by ASCII word boundaries.
pub fn match_plugin_keyword(draft: &str, candidates: &[KeywordCandidate<'_>]) -> Option<usize> {
    if draft.chars().count() < 3 {
        return None;
    }
    let draft_lc = draft.to_ascii_lowercase();
    let haystack = draft_lc.as_bytes();

    let mut pairs: Vec<(String, usize)> = Vec::new();
    for (idx, candidate) in candidates.iter().enumerate() {
        for keyword in effective_keywords(candidate) {
            pairs.push((keyword, idx));
        }
    }
    pairs.sort_by_key(|(keyword, _)| Reverse(keyword.len()));

    pairs
        .iter()
        .find(|(keyword, _)| keyword_matches(haystack, keyword.as_bytes()))
        .map(|(_, idx)| *idx)
}

fn effective_keywords(candidate: &KeywordCandidate<'_>) -> Vec<String> {
    let mut keywords = Vec::new();
    for keyword in candidate.keywords {
        let normalized = keyword.trim().to_ascii_lowercase();
        if !normalized.is_empty() {
            keywords.push(normalized);
        }
    }
    for domain in candidate.domains {
        if let Some(normalized) = normalize_domain(domain) {
            keywords.push(normalized);
        }
    }
    let name = candidate.name.trim().to_ascii_lowercase();
    if !name.is_empty() {
        keywords.push(name);
    }
    keywords
}

pub(crate) fn normalize_domain(domain: &str) -> Option<String> {
    let trimmed = domain.trim();
    let after_scheme = match trimmed.find("://") {
        Some(i) => &trimmed[i + 3..],
        None => trimmed,
    };
    let host = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme)
        .to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn keyword_matches(haystack: &[u8], keyword: &[u8]) -> bool {
    if keyword.is_empty() {
        return false;
    }
    let len = haystack.len();
    haystack
        .windows(keyword.len())
        .enumerate()
        .any(|(start, window)| {
            if window != keyword {
                return false;
            }
            let end = start + keyword.len();
            let start_ok = start == 0 || is_word(haystack[start - 1]) != is_word(haystack[start]);
            let end_ok = end == len || is_word(haystack[end - 1]) != is_word(haystack[end]);
            start_ok && end_ok
        })
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate<'a>(
        name: &'a str,
        domains: &'a [String],
        keywords: &'a [String],
    ) -> KeywordCandidate<'a> {
        KeywordCandidate {
            name,
            domains,
            keywords,
        }
    }

    #[test]
    fn longest_keyword_takes_precedence() {
        let short = vec!["editor".to_string()];
        let long = vec!["code editor".to_string()];
        let candidates = [
            candidate("plugin-a", &[], &short),
            candidate("plugin-b", &[], &long),
        ];
        assert_eq!(
            match_plugin_keyword("my code editor rocks", &candidates),
            Some(1)
        );
    }

    #[test]
    fn word_boundary_required() {
        let keywords = vec!["box".to_string()];
        let candidates = [candidate("box", &[], &keywords)];
        assert_eq!(match_plugin_keyword("i love boxing", &candidates), None);
        assert_eq!(match_plugin_keyword("i love box", &candidates), Some(0));
    }

    #[test]
    fn boxing_does_not_match_box() {
        let keywords = vec!["box".to_string()];
        let candidates = [candidate("box", &[], &keywords)];
        assert_eq!(match_plugin_keyword("try boxing drills", &candidates), None);
    }

    #[test]
    fn domains_match_inside_pasted_urls() {
        let domains = vec!["figma.com".to_string()];
        let none: Vec<String> = Vec::new();
        let candidates = [candidate("design-app", &domains, &none)];
        assert_eq!(
            match_plugin_keyword("open https://www.figma.com/board/x please", &candidates),
            Some(0)
        );
        assert_eq!(
            match_plugin_keyword("open figma.com please", &candidates),
            Some(0)
        );
        assert_eq!(match_plugin_keyword("open figma please", &candidates), None);
    }

    #[test]
    fn name_is_used_as_fallback() {
        let none: Vec<String> = Vec::new();
        let candidates = [candidate("obsidian", &[], &none)];
        assert_eq!(
            match_plugin_keyword("open obsidian now", &candidates),
            Some(0)
        );
    }

    #[test]
    fn draft_below_min_length_never_matches() {
        let keywords = vec!["go".to_string()];
        let candidates = [candidate("go", &[], &keywords)];
        assert_eq!(match_plugin_keyword("go", &candidates), None);
        let git = vec!["git".to_string()];
        let candidates = [candidate("git", &[], &git)];
        assert_eq!(match_plugin_keyword("git", &candidates), Some(0));
    }
}
