//! LRU page bitmap cache with a byte cap (PLAN §4.4).

use std::collections::HashMap;
use std::sync::Arc;

use tiny_skia::Pixmap;

/// Default cap on cached page bitmaps: 64 MB.
pub const DEFAULT_PAGE_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Cache key: (page index, page content hash, scale bits). The scale is the product of zoom
/// and the window scale factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageKey {
    pub page: usize,
    pub content_hash: u64,
    pub scale_bits: u32,
}

struct Entry {
    pixmap: Arc<Pixmap>,
    tick: u64,
    bytes: usize,
}

/// Least-recently-used cache of rendered pages, evicting oldest-first until under the cap.
pub struct PageCache {
    map: HashMap<PageKey, Entry>,
    bytes: usize,
    cap_bytes: usize,
    tick: u64,
}

impl PageCache {
    pub fn new(cap_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            bytes: 0,
            cap_bytes,
            tick: 0,
        }
    }

    pub fn get(&mut self, key: &PageKey) -> Option<Arc<Pixmap>> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(key).map(|e| {
            e.tick = t;
            e.pixmap.clone()
        })
    }

    pub fn insert(&mut self, key: PageKey, pixmap: Arc<Pixmap>) {
        self.tick += 1;
        let bytes = pixmap.data().len();
        if let Some(old) = self.map.insert(
            key,
            Entry {
                pixmap,
                tick: self.tick,
                bytes,
            },
        ) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        // Evict oldest first, but never the entry just inserted.
        while self.bytes > self.cap_bytes && self.map.len() > 1 {
            let oldest = self
                .map
                .iter()
                .filter(|(k, _)| **k != key)
                .min_by_key(|(_, e)| e.tick)
                .map(|(k, _)| *k);
            let Some(k) = oldest else { break };
            if let Some(e) = self.map.remove(&k) {
                self.bytes -= e.bytes;
            }
        }
    }

    /// Drops every cached page (for example on a font or theme change).
    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn cap_bytes(&self) -> usize {
        self.cap_bytes
    }
}

impl Default for PageCache {
    fn default() -> Self {
        Self::new(DEFAULT_PAGE_CACHE_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(i: usize) -> PageKey {
        PageKey {
            page: i,
            content_hash: 0,
            scale_bits: 1.0f32.to_bits(),
        }
    }

    #[test]
    fn evicts_least_recently_used_under_cap() {
        // Each 10x10 pixmap is 400 bytes; cap holds two.
        let mut c = PageCache::new(900);
        let px = |_: usize| Arc::new(Pixmap::new(10, 10).expect("pixmap"));
        c.insert(key(0), px(0));
        c.insert(key(1), px(1));
        assert!(c.get(&key(0)).is_some()); // 0 is now more recent than 1
        c.insert(key(2), px(2));
        assert_eq!(c.len(), 2);
        assert!(c.get(&key(1)).is_none(), "LRU entry should be evicted");
        assert!(c.get(&key(0)).is_some() && c.get(&key(2)).is_some());
        assert!(c.bytes() <= 900);
    }

    #[test]
    fn oversized_entry_is_kept_alone() {
        let mut c = PageCache::new(100);
        c.insert(key(0), Arc::new(Pixmap::new(10, 10).expect("pixmap")));
        assert_eq!(c.len(), 1);
    }
}
