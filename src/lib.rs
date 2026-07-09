use std::collections::HashMap;

pub struct KVStore {
    map: HashMap<String, String>,
}

impl KVStore {
    pub fn new() -> Self {
        KVStore {
            map: HashMap::new(),
        }
    }
    pub fn set(&mut self, key: String, value: String) {
        self.map.insert(key, value);
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(|v| v.as_str())
    }
    pub fn delete(&mut self, key: &str) -> Option<String> {
        self.map.remove(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_roundtrip() {
        let mut store = KVStore::new();
        let key = "K";
        let val = "V";
        store.set(key.into(), val.into());
        assert_eq!(store.get(key), Some(val));
    }

    #[test]
    fn get_empty_value_returns_none() {
        let store = KVStore::new();
        assert_eq!(store.get("K".into()), None);
    }

    #[test]
    fn get_empty_value_is_distinct_from_missing() {
        let mut store = KVStore::new();
        store.set("K".into(), "".into());
        assert_eq!(store.get("K"), Some(""));
        assert_eq!(store.get("absent"), None);
    }

    #[test]
    fn set_delete_roundtrip() {
        let mut store = KVStore::new();
        let key = "K";
        let val = "V";

        store.set(key.into(), val.into());
        assert_eq!(store.delete(key), Some(val.into()));
    }

    #[test]
    fn set_delete_get_returns_none() {
        let mut store = KVStore::new();
        let key = "K";
        let val = "V";

        store.set(key.into(), val.into());
        assert_eq!(store.delete(key), Some(val.to_string()));
        assert_eq!(store.get(key), None);
    }

    #[test]
    fn delete_absent_key_returns_none() {
        let mut store = KVStore::new();
        let key = "K";
        assert_eq!(store.delete(key), None);
    }
}
