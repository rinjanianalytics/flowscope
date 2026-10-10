// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! A map tracking when entries were last accessed and last updated. Port of
//! `common/helpers/cache`. It can be saved to a file (as JSON; the Go
//! version uses gob, the two files are not interchangeable).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::RwLock;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since epoch of a time.
pub fn unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

struct Item<V> {
    object: V,
    last_accessed: AtomicI64,
    last_updated: i64,
}

/// The cache.
pub struct Cache<K, V> {
    items: RwLock<HashMap<K, Item<V>>>,
}

impl<K, V> Default for Cache<K, V> {
    fn default() -> Self {
        Cache { items: RwLock::new(HashMap::new()) }
    }
}

/// Version of the persisted format.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Persisted<K, V> {
    version: u32,
    items: Vec<(K, V, i64, i64)>,
}

impl<K: Eq + Hash + Clone, V: Clone> Cache<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Put an entry.
    pub fn put(&self, now: SystemTime, key: K, object: V) {
        let n = unix(now);
        self.items.write().unwrap().insert(key, Item { object, last_accessed: AtomicI64::new(n), last_updated: n });
    }

    /// Get an entry and update its access time.
    pub fn get(&self, now: SystemTime, key: &K) -> Option<V> {
        let items = self.items.read().unwrap();
        let item = items.get(key)?;
        item.last_accessed.store(unix(now), Ordering::Relaxed);
        Some(item.object.clone())
    }

    /// Entries updated before the provided time.
    pub fn items_last_updated_before(&self, before: SystemTime) -> Vec<(K, V)> {
        let b = unix(before);
        self.items
            .read()
            .unwrap()
            .iter()
            .filter(|(_, v)| v.last_updated < b)
            .map(|(k, v)| (k.clone(), v.object.clone()))
            .collect()
    }

    /// Delete entries not accessed since the provided time.
    pub fn delete_last_accessed_before(&self, before: SystemTime) -> usize {
        let b = unix(before);
        let mut items = self.items.write().unwrap();
        let n = items.len();
        items.retain(|_, v| v.last_accessed.load(Ordering::Relaxed) >= b);
        n - items.len()
    }

    pub fn len(&self) -> usize {
        self.items.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<K, V> Cache<K, V>
where
    K: Eq + Hash + Clone + Serialize + DeserializeOwned,
    V: Clone + Serialize + DeserializeOwned,
{
    /// Save to a file, atomically.
    pub fn save(&self, path: &str) -> Result<(), String> {
        let items: Vec<(K, V, i64, i64)> = self
            .items
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.object.clone(), v.last_accessed.load(Ordering::Relaxed), v.last_updated))
            .collect();
        let data = serde_json::to_vec(&Persisted { version: VERSION, items })
            .map_err(|e| format!("unable to encode cache: {e}"))?;
        let tmp = format!("{path}.tmp");
        std::fs::write(&tmp, data)
            .and_then(|_| std::fs::rename(&tmp, path))
            .map_err(|e| format!("unable to write cache file {path:?}: {e}"))
    }

    /// Load from a file.
    pub fn load(&self, path: &str) -> Result<(), String> {
        let data = std::fs::read(path).map_err(|e| format!("unable to load cache {path:?}: {e}"))?;
        let p: Persisted<K, V> = serde_json::from_slice(&data).map_err(|e| format!("unable to decode cache: {e}"))?;
        if p.version != VERSION {
            return Err("cache version mismatch".into());
        }
        let mut items = self.items.write().unwrap();
        for (k, object, accessed, updated) in p.items {
            items.insert(k, Item { object, last_accessed: AtomicI64::new(accessed), last_updated: updated });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn expire() {
        let c: Cache<String, u32> = Cache::new();
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        c.put(t0, "a".into(), 1);
        c.put(t0, "b".into(), 2);
        assert_eq!(c.get(t0 + Duration::from_secs(100), &"a".into()), Some(1));
        assert_eq!(c.items_last_updated_before(t0 + Duration::from_secs(1)).len(), 2);
        assert_eq!(c.delete_last_accessed_before(t0 + Duration::from_secs(50)), 1);
        assert_eq!(c.get(t0, &"b".into()), None);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn persist() {
        let dir = std::env::temp_dir().join(format!("flowscope-cache-{}", std::process::id()));
        let path = dir.to_string_lossy().to_string();
        let c: Cache<(String, u32), String> = Cache::new();
        c.put(SystemTime::now(), ("a".into(), 1), "x".into());
        c.save(&path).unwrap();
        let d: Cache<(String, u32), String> = Cache::new();
        d.load(&path).unwrap();
        assert_eq!(d.get(SystemTime::now(), &("a".into(), 1)), Some("x".into()));
        std::fs::remove_file(&path).unwrap();
    }
}
