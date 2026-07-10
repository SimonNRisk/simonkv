use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};

pub struct KVStore {
    map: HashMap<String, String>,
    log: File,
}

impl KVStore {
    pub fn open(path: &str) -> io::Result<Self> {
        // io::Result shorthand for Result<T, std::io::Error>. Tells user it fails at IO
        let log = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(KVStore {
            map: HashMap::new(),
            log,
        })
    }
    pub fn set(&mut self, key: String, value: String) -> io::Result<()> {
        writeln!(self.log, "SET {} {}", key, value)?;
        self.map.insert(key, value);
        Ok(())
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(|v| v.as_str())
    }
    pub fn delete(&mut self, key: &str) -> io::Result<Option<String>> {
        writeln!(self.log, "DEL {}", key)?;
        Ok(self.map.remove(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn make_store() -> (KVStore, NamedTempFile) {
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap();
        let store = KVStore::open(&path).unwrap();
        (store, file)
    }

    #[test]
    fn set_get_roundtrip() {
        let (mut store, _) = make_store();
        let key = "K";
        let val = "V";
        store.set(key.into(), val.into()).unwrap();
        assert_eq!(store.get(key), Some(val));
    }

    #[test]
    fn get_empty_value_returns_none() {
        let (store, _) = make_store();
        assert_eq!(store.get("K".into()), None);
    }

    #[test]
    fn get_empty_value_is_distinct_from_missing() {
        let (mut store, _) = make_store();
        store.set("K".into(), "".into()).unwrap();
        assert_eq!(store.get("K"), Some(""));
        assert_eq!(store.get("absent"), None);
    }

    #[test]
    fn set_delete_roundtrip() {
        let (mut store, _) = make_store();
        let key = "K";
        let val = "V";

        store.set(key.into(), val.into()).unwrap();
        assert_eq!(store.delete(key).unwrap(), Some(val.into()));
    }

    #[test]
    fn set_delete_get_returns_none() {
        let (mut store, _) = make_store();
        let key = "K";
        let val = "V";

        store.set(key.into(), val.into()).unwrap();
        assert_eq!(store.delete(key).unwrap(), Some(val.to_string()));
        assert_eq!(store.get(key), None);
    }

    #[test]
    fn delete_absent_key_returns_none() {
        let (mut store, _) = make_store();
        let key = "K";
        assert_eq!(store.delete(key).unwrap(), None);
    }
}
