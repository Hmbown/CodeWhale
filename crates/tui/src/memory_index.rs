use std::collections::{HashMap, HashSet};

use crate::memory::MemoryEntry;

/// Lightweight inverted index over memory entries.
///
/// Maintains two indices:
/// - **Tag index**: maps each tag (lowercased, without `#`) to the set of
///   entry indices that carry it.
/// - **Full-text index**: maps each word (lowercased) to the set of entry
///   indices whose body or tags contain it.
///
/// The index is rebuilt from scratch each time the memory file changes,
/// keeping the implementation simple and avoiding stale-entry bugs.
pub struct MemoryIndex {
    /// Entries in display order (oldest first).
    entries: Vec<MemoryEntry>,
    /// Inverted index: tag → entry indices.
    tag_index: HashMap<String, Vec<usize>>,
    /// Full-text index: word → entry indices.
    text_index: HashMap<String, Vec<usize>>,
}

impl MemoryIndex {
    /// Build an index from parsed memory entries.
    #[must_use]
    pub fn build(entries: Vec<MemoryEntry>) -> Self {
        let mut tag_index: HashMap<String, Vec<usize>> = HashMap::new();
        let mut text_index: HashMap<String, Vec<usize>> = HashMap::new();

        for (i, entry) in entries.iter().enumerate() {
            // Index tags
            for tag in &entry.tags {
                let key = tag.to_lowercase();
                tag_index.entry(key).or_default().push(i);
            }
            // Index body words
            for word in entry.body.split_whitespace() {
                let clean: String = word
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                if clean.len() >= 2 {
                    text_index.entry(clean.to_lowercase()).or_default().push(i);
                }
            }
            // Index tag strings as text too
            for tag in &entry.tags {
                for word in tag.split(|c: char| !c.is_alphanumeric()) {
                    if word.len() >= 2 {
                        text_index
                            .entry(word.to_lowercase())
                            .or_default()
                            .push(i);
                    }
                }
            }
        }

        // Deduplicate index entries (same entry may contribute a word multiple times)
        for indices in tag_index.values_mut() {
            indices.sort_unstable();
            indices.dedup();
        }
        for indices in text_index.values_mut() {
            indices.sort_unstable();
            indices.dedup();
        }

        Self {
            entries,
            tag_index,
            text_index,
        }
    }

    /// Rebuild the index from memory file content.
    #[must_use]
    pub fn from_content(content: &str) -> Self {
        Self::build(crate::memory::parse_all(content))
    }

    /// Return a reference to the underlying entries.
    #[must_use]
    pub fn entries(&self) -> &[MemoryEntry] {
        &self.entries
    }

    /// Search by tags (OR logic — any matching tag). Returns matching
    /// entries in display order.
    #[must_use]
    pub fn search_by_tags(&self, tags: &[&str]) -> Vec<&MemoryEntry> {
        if tags.is_empty() {
            return self.entries.iter().collect();
        }
        let mut matched = HashSet::new();
        for tag in tags {
            let key = tag.trim_start_matches('#').to_lowercase();
            if let Some(indices) = self.tag_index.get(&key) {
                for &i in indices {
                    matched.insert(i);
                }
            }
        }
        let mut indices: Vec<usize> = matched.into_iter().collect();
        indices.sort_unstable();
        indices.iter().map(|&i| &self.entries[i]).collect()
    }

    /// Full-text search (AND logic — all query words must match). Returns
    /// matching entries in display order.
    #[must_use]
    pub fn search_text(&self, query: &str) -> Vec<&MemoryEntry> {
        let words: Vec<String> = query
            .split_whitespace()
            .filter_map(|w| {
                let clean: String = w
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                if clean.len() >= 2 {
                    Some(clean.to_lowercase())
                } else {
                    None
                }
            })
            .collect();

        if words.is_empty() {
            return self.entries.iter().collect();
        }

        // Find intersection of all word matches
        let mut result: Option<HashSet<usize>> = None;
        for word in &words {
            if let Some(indices) = self.text_index.get(word) {
                let set: HashSet<usize> = indices.iter().copied().collect();
                result = match result {
                    Some(existing) => Some(existing.intersection(&set).copied().collect()),
                    None => Some(set),
                };
            } else {
                // A required word has no matches → empty result
                return Vec::new();
            }
        }

        let mut indices: Vec<usize> = result.unwrap_or_default().into_iter().collect();
        indices.sort_unstable();
        indices.iter().map(|&i| &self.entries[i]).collect()
    }

    /// Combined search: filter by tags (OR) and text (AND).
    /// Returns entries that match both criteria.
    #[must_use]
    pub fn search(&self, tags: &[&str], text: Option<&str>) -> Vec<&MemoryEntry> {
        if tags.is_empty() && text.is_none() {
            return self.entries.iter().collect();
        }

        let entry_set: HashSet<usize> = (0..self.entries.len()).collect();

        let tag_indices: HashSet<usize> = if tags.is_empty() {
            entry_set.clone()
        } else {
            let mut s = HashSet::new();
            for tag in tags {
                let key = tag.trim_start_matches('#').to_lowercase();
                if let Some(indices) = self.tag_index.get(&key) {
                    for &i in indices {
                        s.insert(i);
                    }
                }
            }
            s
        };

        let text_indices: HashSet<usize> = if let Some(query) = text {
            let words: Vec<String> = query
                .split_whitespace()
                .filter_map(|w| {
                    let clean: String = w
                        .chars()
                        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                        .collect();
                    if clean.len() >= 2 {
                        Some(clean.to_lowercase())
                    } else {
                        None
                    }
                })
                .collect();
            if words.is_empty() {
                entry_set
            } else {
                let mut result: Option<HashSet<usize>> = None;
                for word in &words {
                    if let Some(indices) = self.text_index.get(word) {
                        let set: HashSet<usize> = indices.iter().copied().collect();
                        result = Some(match result {
                            Some(existing) => existing.intersection(&set).copied().collect(),
                            None => set,
                        });
                    } else {
                        return Vec::new();
                    }
                }
                result.unwrap_or_default()
            }
        } else {
            entry_set
        };

        let mut indices: Vec<usize> = tag_indices
            .intersection(&text_indices)
            .copied()
            .collect();
        indices.sort_unstable();
        indices.iter().map(|&i| &self.entries[i]).collect()
    }

    /// Get all unique tags with their occurrence counts, sorted by
    /// frequency (most frequent first).
    #[must_use]
    pub fn all_tags(&self) -> Vec<(String, usize)> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for (_tag, indices) in &self.tag_index {
            counts.insert(_tag.clone(), indices.len());
        }
        let mut result: Vec<_> = counts.into_iter().collect();
        result.sort_by(|a, b| b.1.cmp(&a.1));
        result
    }

    /// Number of entries in the index.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<MemoryEntry> {
        crate::memory::parse_all(
            "\
- (2026-06-22 10:00 UTC) first entry about Rust #rust
- (2026-06-22 11:00 UTC) python web framework #python #web
- (2026-06-22 12:00 UTC) rust cli tooling #rust #cli
- (2026-06-22 13:00 UTC) web design patterns #web",
        )
    }

    #[test]
    fn index_builds_from_entries() {
        let index = MemoryIndex::build(sample_entries());
        assert_eq!(index.len(), 4);
        assert!(!index.is_empty());
    }

    #[test]
    fn index_from_content_parses_and_indexes() {
        let index = MemoryIndex::from_content(
            "- (2026-06-22 10:00 UTC) test entry #test",
        );
        assert_eq!(index.len(), 1);
        let tags = index.all_tags();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].0, "test");
    }

    #[test]
    fn search_by_tags_or() {
        let index = MemoryIndex::build(sample_entries());
        let results = index.search_by_tags(&["rust"]);
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|e| e.body.contains("first entry")));
        assert!(results.iter().any(|e| e.body.contains("cli tooling")));
    }

    #[test]
    fn search_by_tags_multiple() {
        let index = MemoryIndex::build(sample_entries());
        let results = index.search_by_tags(&["python", "cli"]);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_by_tags_empty_returns_all() {
        let index = MemoryIndex::build(sample_entries());
        let results = index.search_by_tags(&[]);
        assert_eq!(results.len(), 4);
    }

    #[test]
    fn search_text_and() {
        let index = MemoryIndex::build(sample_entries());
        // "rust framework" → intersection: entry 0 has "rust", entry 1 has "framework"
        // None should have both
        let results = index.search_text("rust framework");
        assert!(results.is_empty());
    }

    #[test]
    fn search_text_single_word() {
        let index = MemoryIndex::build(sample_entries());
        let results = index.search_text("python");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_text_case_insensitive() {
        let index = MemoryIndex::build(sample_entries());
        let results = index.search_text("RUST");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_combined() {
        let index = MemoryIndex::build(sample_entries());
        let results = index.search(&["web"], Some("patterns"));
        assert_eq!(results.len(), 1);
        assert!(results[0].body.contains("design patterns"));
    }

    #[test]
    fn search_no_match() {
        let index = MemoryIndex::build(sample_entries());
        let results = index.search(&["nonexistent"], None);
        assert!(results.is_empty());
    }

    #[test]
    fn all_tags_sorted_by_frequency() {
        let index = MemoryIndex::build(sample_entries());
        let tags = index.all_tags();
        assert!(tags.iter().any(|(t, _)| t == "rust"));
        assert!(tags.iter().any(|(t, _)| t == "web"));
        assert!(tags.iter().any(|(t, _)| t == "python"));
        assert!(tags.iter().any(|(t, _)| t == "cli"));
    }

    #[test]
    fn empty_index() {
        let index = MemoryIndex::build(vec![]);
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
        assert!(index.all_tags().is_empty());
        assert!(index.search_by_tags(&["anything"]).is_empty());
        assert!(index.search_text("anything").is_empty());
    }
}
