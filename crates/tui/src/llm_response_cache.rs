//! In-process LLM response-level LRU cache for deduplicating identical requests.
//!
//! When the same request is sent multiple times (e.g., model retries after
//! network errors, app-server batch processing, or user resubmissions), this
//! cache returns the cached response immediately without making an API call.
//!
//! **Scope**: Only non-streaming, tool-free requests are cached. Streaming
//! responses and tool-carrying requests are excluded to avoid complexity and
//! side-effect issues.
//!
//! **Cache key**: SHA-256 of the *canonical wire body* — the final JSON
//! object that would be sent to the chat completions API, after all
//! provider/model normalizations, reasoning-effort transformations, tool
//! sanitization, and max_tokens adjustments have been applied. This
//! ensures that any transformation applied to the request is reflected in
//! the key, preventing false cache hits on requests that differ only in
//! their post-processing.
//!
//! **Value**: Complete `MessageResponse` including usage tokens, so cost
//! tracking remains accurate on cache hits.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use lru::LruCache;
use sha2::{Digest, Sha256};

use crate::models::MessageResponse;

/// Default cache capacity: 256 entries (~1 MB at 4 KB/response average).
const DEFAULT_CAPACITY: usize = 256;

/// Global response cache singleton.
static RESPONSE_CACHE: OnceLock<LlmResponseCache> = OnceLock::new();

/// Get or initialize the global response cache.
pub fn response_cache() -> &'static LlmResponseCache {
    RESPONSE_CACHE.get_or_init(LlmResponseCache::new)
}

/// Per-request cache statistics snapshot.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub size: usize,
    pub capacity: usize,
}

/// In-process LRU cache for LLM responses.
pub struct LlmResponseCache {
    inner: Mutex<LruCache<[u8; 32], MessageResponse>>,
    hits: AtomicU64,
    misses: AtomicU64,
    capacity: usize,
}

impl LlmResponseCache {
    /// Create a cache with the default capacity (256 entries).
    pub fn new() -> Self {
        Self::with_capacity(NonZeroUsize::new(DEFAULT_CAPACITY).unwrap())
    }

    /// Create a cache with the specified capacity.
    pub fn with_capacity(cap: NonZeroUsize) -> Self {
        Self {
            inner: Mutex::new(LruCache::new(cap)),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            capacity: cap.get(),
        }
    }

    /// Compute a cache key from the canonical wire body bytes.
    ///
    /// The key is SHA-256 of the final JSON body that would be sent to the
    /// chat completions API. Using the wire body (rather than individual
    /// pre-transformation fields) ensures that any provider-specific
    /// normalization, reasoning-effort mapping, tool sanitization, or
    /// max_tokens adjustment is reflected in the key.
    pub fn make_key(wire_body: &[u8]) -> [u8; 32] {
        Sha256::digest(wire_body).into()
    }

    /// Look up a cached response.
    pub fn get(&self, key: &[u8; 32]) -> Option<MessageResponse> {
        let mut cache = self.inner.lock().unwrap();
        if let Some(response) = cache.get(key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            Some(response.clone())
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Insert a response into the cache.
    pub fn put(&self, key: [u8; 32], value: MessageResponse) {
        let mut cache = self.inner.lock().unwrap();
        cache.put(key, value);
    }

    /// Return the cache hit rate as a fraction in [0.0, 1.0].
    #[allow(dead_code)]
    pub fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }

    /// Return a snapshot of cache statistics.
    #[allow(dead_code)]
    pub fn stats(&self) -> CacheStats {
        let cache = self.inner.lock().unwrap();
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            size: cache.len(),
            capacity: self.capacity,
        }
    }

    /// Reset hit/miss counters (for testing).
    #[cfg(test)]
    #[allow(dead_code)]
    fn reset_counters(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
    }
}

impl Default for LlmResponseCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Usage;

    fn make_response(id: &str) -> MessageResponse {
        MessageResponse {
            id: id.to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![],
            model: "test-model".to_string(),
            stop_reason: Some("end_turn".to_string()),
            stop_sequence: None,
            container: None,
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                prompt_cache_hit_tokens: None,
                prompt_cache_miss_tokens: None,
                reasoning_tokens: None,
                reasoning_replay_tokens: None,
                server_tool_use: None,
            },
        }
    }

    #[test]
    fn make_key_different_inputs_produce_different_keys() {
        let key1 = LlmResponseCache::make_key(
            b"{\"model\":\"v4\",\"messages\":[{\"role\":\"user\",\"content\":\"msg1\"}]}",
        );
        let key2 = LlmResponseCache::make_key(
            b"{\"model\":\"v4\",\"messages\":[{\"role\":\"user\",\"content\":\"msg2\"}]}",
        );
        let key3 = LlmResponseCache::make_key(
            b"{\"model\":\"other\",\"messages\":[{\"role\":\"user\",\"content\":\"msg1\"}]}",
        );
        assert_ne!(key1, key2, "different bodies should produce different keys");
        assert_ne!(key1, key3, "different models should produce different keys");
    }

    #[test]
    fn make_key_same_input_produces_same_key() {
        let body = b"{\"model\":\"v4\",\"messages\":[{\"role\":\"user\",\"content\":\"msg\"}]}";
        let key1 = LlmResponseCache::make_key(body);
        let key2 = LlmResponseCache::make_key(body);
        assert_eq!(key1, key2, "same body should produce same key");
    }

    #[test]
    fn make_key_reflects_transformations() {
        // Two bodies that differ only in reasoning_effort should produce
        // different keys. This is the primary correctness property the
        // wire-body key design provides.
        let base = b"{\"model\":\"v4\",\"messages\":[]}";
        let with_effort = b"{\"model\":\"v4\",\"messages\":[],\"reasoning_effort\":\"high\"}";
        let k1 = LlmResponseCache::make_key(base);
        let k2 = LlmResponseCache::make_key(with_effort);
        assert_ne!(k1, k2, "reasoning_effort difference must change the key");
    }

    #[test]
    fn put_and_get_returns_cached_response() {
        let cache = LlmResponseCache::new();
        let key = LlmResponseCache::make_key(b"msg");
        let response = make_response("resp1");

        cache.put(key, response.clone());
        let cached = cache.get(&key).unwrap();
        assert_eq!(cached.id, "resp1");
    }

    #[test]
    fn get_miss_returns_none() {
        let cache = LlmResponseCache::new();
        let key = LlmResponseCache::make_key(b"msg");
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn capacity_evicts_oldest() {
        let cache = LlmResponseCache::with_capacity(NonZeroUsize::new(2).unwrap());

        let key1 = LlmResponseCache::make_key(b"msg1");
        let key2 = LlmResponseCache::make_key(b"msg2");
        let key3 = LlmResponseCache::make_key(b"msg3");

        cache.put(key1, make_response("r1"));
        cache.put(key2, make_response("r2"));
        cache.put(key3, make_response("r3"));

        // key1 should be evicted.
        assert!(cache.get(&key1).is_none(), "oldest entry should be evicted");
        assert!(
            cache.get(&key2).is_some(),
            "second entry should still be present"
        );
        assert!(
            cache.get(&key3).is_some(),
            "newest entry should still be present"
        );
    }

    #[test]
    fn hit_rate_tracks_correctly() {
        let cache = LlmResponseCache::new();
        let key = LlmResponseCache::make_key(b"msg");

        cache.put(key, make_response("r1"));
        cache.get(&key); // hit
        cache.get(&key); // hit
        let miss_key = LlmResponseCache::make_key(b"other");
        cache.get(&miss_key); // miss

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert!((cache.hit_rate() - 2.0 / 3.0).abs() < 0.001);
    }

    #[test]
    fn stats_reports_size_and_capacity() {
        let cache = LlmResponseCache::with_capacity(NonZeroUsize::new(10).unwrap());
        let key1 = LlmResponseCache::make_key(b"msg1");
        let key2 = LlmResponseCache::make_key(b"msg2");

        cache.put(key1, make_response("r1"));
        cache.put(key2, make_response("r2"));

        let stats = cache.stats();
        assert_eq!(stats.size, 2);
        assert_eq!(stats.capacity, 10);
    }
}
