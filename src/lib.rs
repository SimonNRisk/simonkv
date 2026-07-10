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

    fn make_store(filename: &str) -> KVStore {
        let path = format!("{}.log", filename);
        let _ = std::fs::remove_file(&path);
        KVStore::open(&path).unwrap()
    }

    #[test]
    fn set_get_roundtrip() {
        let mut store = make_store("set_get_roundtrip");
        let key = "K";
        let val = "V";
        let _ = store.set(key.into(), val.into());
        assert_eq!(store.get(key), Some(val));
    }

    #[test]
    fn get_empty_value_returns_none() {
        let store = make_store("get_empty_value_returns_none");
        assert_eq!(store.get("K".into()), None);
    }

    #[test]
    fn get_empty_value_is_distinct_from_missing() {
        let mut store = make_store("get_empty_value_is_distinct_from_missing");
        let _ = store.set("K".into(), "".into());
        assert_eq!(store.get("K"), Some(""));
        assert_eq!(store.get("absent"), None);
    }

    #[test]
    fn set_delete_roundtrip() {
        let mut store = make_store("set_delete_roundtrip");
        let key = "K";
        let val = "V";

        let _ = store.set(key.into(), val.into());
        assert_eq!(store.delete(key).unwrap(), Some(val.into()));
    }

    #[test]
    fn set_delete_get_returns_none() {
        let mut store = make_store("set_delete_get_returns_none");
        let key = "K";
        let val = "V";

        let _ = store.set(key.into(), val.into());
        assert_eq!(store.delete(key).unwrap(), Some(val.to_string()));
        assert_eq!(store.get(key), None);
    }

    #[test]
    fn delete_absent_key_returns_none() {
        let mut store = make_store("set_dedelete_absent_key_returns_noneete_get_returns_none");
        let key = "K";
        assert_eq!(store.delete(key).unwrap(), None);
    }
}
