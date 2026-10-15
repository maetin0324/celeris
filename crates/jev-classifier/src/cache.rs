use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// 入力 hash をキーにした bounded cache（FIFO 追い出し）。0 件なら無効。
#[derive(Debug)]
pub struct BoundedCache<V: Clone> {
    max: usize,
    inner: Mutex<Inner<V>>,
}

#[derive(Debug)]
struct Inner<V> {
    map: HashMap<String, V>,
    order: VecDeque<String>,
}

impl<V: Clone> BoundedCache<V> {
    pub fn new(max: usize) -> Self {
        Self {
            max,
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    pub fn get(&self, key: &str) -> Option<V> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map
            .get(key)
            .cloned()
    }

    pub fn put(&self, key: String, value: V) {
        if self.max == 0 {
            return;
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.map.insert(key.clone(), value).is_none() {
            g.order.push_back(key);
        }
        while g.map.len() > self.max {
            match g.order.pop_front() {
                Some(old) => {
                    g.map.remove(&old);
                }
                None => break,
            }
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map
            .len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_beyond_bound() {
        let c = BoundedCache::new(2);
        c.put("a".into(), 1);
        c.put("b".into(), 2);
        c.put("c".into(), 3);
        assert_eq!(c.len(), 2);
        assert_eq!(c.get("a"), None);
        assert_eq!(c.get("c"), Some(3));
    }

    #[test]
    fn zero_disables() {
        let c = BoundedCache::new(0);
        c.put("a".into(), 1);
        assert!(c.is_empty());
    }
}
