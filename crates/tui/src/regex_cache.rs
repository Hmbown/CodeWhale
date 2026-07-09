use std::num::NonZeroUsize;
use std::sync::{Mutex, OnceLock};

use lru::LruCache;
use regex::Regex;

const DEFAULT_USER_REGEX_CACHE_CAPACITY: usize = 64;

static USER_REGEX_CACHE: OnceLock<UserRegexCache> = OnceLock::new();

pub(crate) fn compile_user_regex(pattern: &str) -> Result<Regex, regex::Error> {
    user_regex_cache().compile(pattern)
}

fn user_regex_cache() -> &'static UserRegexCache {
    USER_REGEX_CACHE.get_or_init(UserRegexCache::new)
}

struct UserRegexCache {
    inner: Mutex<LruCache<String, Regex>>,
}

impl UserRegexCache {
    fn new() -> Self {
        Self::with_capacity(
            NonZeroUsize::new(DEFAULT_USER_REGEX_CACHE_CAPACITY).expect("non-zero capacity"),
        )
    }

    fn with_capacity(capacity: NonZeroUsize) -> Self {
        Self {
            inner: Mutex::new(LruCache::new(capacity)),
        }
    }

    fn compile(&self, pattern: &str) -> Result<Regex, regex::Error> {
        let Ok(mut cache) = self.inner.lock() else {
            return Regex::new(pattern);
        };
        if let Some(regex) = cache.get(pattern) {
            return Ok(regex.clone());
        }

        let regex = Regex::new(pattern)?;
        cache.put(pattern.to_string(), regex.clone());
        Ok(regex)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().expect("cache lock").len()
    }

    #[cfg(test)]
    fn contains(&self, pattern: &str) -> bool {
        self.inner.lock().expect("cache lock").contains(pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_pattern_uses_one_cache_entry() {
        let cache = UserRegexCache::with_capacity(NonZeroUsize::new(2).unwrap());

        let first = cache.compile("alpha|beta").expect("regex compiles");
        let second = cache.compile("alpha|beta").expect("regex cache hit");

        assert!(first.is_match("alpha"));
        assert!(second.is_match("beta"));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn capacity_evicts_least_recently_used_pattern() {
        let cache = UserRegexCache::with_capacity(NonZeroUsize::new(2).unwrap());

        cache.compile("one").expect("one compiles");
        cache.compile("two").expect("two compiles");
        cache.compile("one").expect("one is refreshed");
        cache.compile("three").expect("three compiles");

        assert!(cache.contains("one"));
        assert!(!cache.contains("two"));
        assert!(cache.contains("three"));
    }

    #[test]
    fn invalid_pattern_is_not_cached() {
        let cache = UserRegexCache::with_capacity(NonZeroUsize::new(2).unwrap());

        assert!(cache.compile("[").is_err());

        assert_eq!(cache.len(), 0);
    }
}
