//! Prompt cache for `/variants` and `/retry`.
//! Small LRU (32 entries) keyed by `(model, hash(system || last_4_turns))`.
//! Never used in the main agent loop to avoid staleness.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

const CAPACITY: usize = 32;

#[derive(Debug, Default)]
pub struct PromptCache {
    map: HashMap<String, String>,
    order: VecDeque<String>,
}

impl PromptCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.map.get(key)
    }

    pub fn insert(&mut self, key: String, prompt: String) {
        if self.map.contains_key(&key) {
            self.map.insert(key.clone(), prompt);
            // Move key to back (most recent)
            self.order.retain(|k| k != &key);
            self.order.push_back(key);
            return;
        }
        if self.map.len() >= CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, prompt);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}

fn hash_system_and_turns(system: &str, last_4: &[String]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    system.hash(&mut hasher);
    for t in last_4 {
        t.hash(&mut hasher);
    }
    hasher.finish()
}

/// Builds a cache key for `(model, system, last_4_turns)`.
pub fn cache_key(model: &str, system: &str, last_4: &[String]) -> String {
    let h = hash_system_and_turns(system, last_4);
    format!("{model}:{h:x}")
}

static GLOBAL: std::sync::OnceLock<Mutex<PromptCache>> = std::sync::OnceLock::new();

fn global() -> &'static Mutex<PromptCache> {
    GLOBAL.get_or_init(|| Mutex::new(PromptCache::new()))
}

/// Global get — used by `/variants` / `/retry`.
pub fn global_get(model: &str, system: &str, last_4: &[String]) -> Option<String> {
    let key = cache_key(model, system, last_4);
    let guard = global().lock().ok()?;
    guard.get(&key).cloned()
}

/// Global insert — used by `/variants` / `/retry`.
pub fn global_insert(model: &str, system: &str, last_4: &[String], prompt: String) {
    let key = cache_key(model, system, last_4);
    if let Ok(mut guard) = global().lock() {
        guard.insert(key, prompt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_evicts_oldest() {
        let mut c = PromptCache::new();
        for i in 0..CAPACITY + 5 {
            c.insert(format!("k{i}"), format!("v{i}"));
        }
        assert_eq!(c.len(), CAPACITY);
        assert!(c.get("k0").is_none());
        assert!(c.get(&format!("k{}", CAPACITY + 4)).is_some());
    }

    #[test]
    fn cache_key_is_stable() {
        let a = cache_key("m", "sys", &["a".to_owned(), "b".to_owned()]);
        let b = cache_key("m", "sys", &["a".to_owned(), "b".to_owned()]);
        assert_eq!(a, b);
        let c = cache_key("m2", "sys", &["a".to_owned()]);
        assert_ne!(a, c);
    }
}
