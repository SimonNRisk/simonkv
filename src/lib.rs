use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

pub struct KVStore {
    map: HashMap<String, String>,
    log: File,
}

enum Command {
    Set(String, String),
    Delete(String),
}

const HEADER_LEN: usize = 2 + 2 + 1;
const SET_TAG: u8 = 0x01; // Just a hex representation for 1
const DELETE_TAG: u8 = 0x02; // Just a hex representation for 2

impl KVStore {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)?;

        let mut map = HashMap::new();
        let reader = BufReader::new(&log);

        for line in reader.lines() {
            let line = line?;
            let command = Self::parse_line(&line)?;

            match command {
                Command::Set(key, value) => {
                    map.insert(key, value);
                }
                Command::Delete(key) => {
                    map.remove(&key);
                }
            }
        }
        Ok(KVStore { map, log })
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
    fn parse_line(line: &str) -> io::Result<Command> {
        let words: Vec<&str> = line.split_whitespace().collect();

        if words.len() == 3 && words[0] == "SET" {
            return Ok(Command::Set(words[1].to_string(), words[2].to_string()));
        }

        if words.len() == 2 && words[0] == "DEL" {
            return Ok(Command::Delete(words[1].to_string()));
        }

        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed log record",
        ))
    }
    fn encode_header(operation: u8, key_len: u16, value_len: u16) -> [u8; HEADER_LEN] {
        let mut header = [0u8; HEADER_LEN];

        header[0] = operation;

        header[1..3].copy_from_slice(&key_len.to_be_bytes());
        header[3..5].copy_from_slice(&value_len.to_be_bytes());

        header
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn make_store() -> (KVStore, NamedTempFile) {
        let file = NamedTempFile::new().unwrap();
        let store = KVStore::open(file.path()).unwrap();
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

    #[test]
    fn set_writes_to_log() {
        let (mut store, file) = make_store();

        store.set("K".into(), "V".into()).unwrap();

        drop(store);

        let contents = std::fs::read_to_string(file.path()).unwrap();
        assert_eq!(contents, "SET K V\n")
    }

    #[test]
    fn del_writes_to_log() {
        let (mut store, file) = make_store();

        store.delete("K".into()).unwrap();

        drop(store);

        let contents = std::fs::read_to_string(file.path()).unwrap();
        assert_eq!(contents, "DEL K\n")
    }

    #[test]
    fn set_del_roundtrip_writes_to_log() {
        let (mut store, file) = make_store();

        store.set("K".into(), "V".into()).unwrap();
        store.delete("K").unwrap();

        drop(store);

        let contents = std::fs::read_to_string(file.path()).unwrap();
        assert_eq!(contents, "SET K V\nDEL K\n")
    }

    #[test]
    fn open_restores_map_from_log() {
        let (mut store, file) = make_store();

        store.set("K".into(), "V".into()).unwrap();

        drop(store);

        let store = KVStore::open(file.path()).unwrap();
        assert_eq!(store.get("K"), Some("V"));
    }

    #[test]
    fn open_stores_most_recent_set_from_log() {
        let (mut store, file) = make_store();

        store.set("K".into(), "V".into()).unwrap();
        store.set("K".into(), "V2".into()).unwrap();

        drop(store);

        let store = KVStore::open(file.path()).unwrap();
        assert_eq!(store.get("K"), Some("V2"));
    }

    #[test]
    fn open_restores_most_recent_data_set_delete() {
        let (mut store, file) = make_store();

        store.set("K".into(), "V".into()).unwrap();
        store.set("K2".into(), "V2".into()).unwrap();

        store.delete("K").unwrap();

        drop(store);

        let store = KVStore::open(file.path()).unwrap();
        assert_eq!(store.get("K"), None);
        assert_eq!(store.get("K2"), Some("V2"));
    }

    #[test]
    fn malformed_command_returns_error() {
        let (store, mut file) = make_store();
        let key = String::from("K");
        let value = String::from("V");

        writeln!(file, "BAD {} {}", &key, &value).unwrap();

        drop(store);

        let result = KVStore::open(file.path());

        assert!(result.is_err());

        let error = match KVStore::open(file.path()) {
            Ok(_) => panic!("expected malformed log to fail"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn missing_set_argument_returns_err() {
        let (store, mut file) = make_store();
        let key = String::from("K");

        writeln!(file, "SET {}", &key).unwrap();

        drop(store);

        let result = KVStore::open(file.path());

        assert!(result.is_err());

        let error = match KVStore::open(file.path()) {
            Ok(_) => panic!("expected malformed log to fail"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn missing_delete_argument_returns_err() {
        let (store, mut file) = make_store();

        writeln!(file, "DEL").unwrap();

        drop(store);

        let result = KVStore::open(file.path());

        assert!(result.is_err());

        let error = match KVStore::open(file.path()) {
            Ok(_) => panic!("expected malformed log to fail"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn valid_recorded_followed_by_malformed_record_returns_err() {
        let (store, mut file) = make_store();

        let key1 = String::from("K1");
        let value1 = String::from("V1");
        writeln!(file, "SET {} {}", &key1, &value1).unwrap();

        let key2 = String::from("K2");
        writeln!(file, "SET {}", &key2).unwrap();

        drop(store);

        let result = KVStore::open(file.path());

        assert!(result.is_err());

        let error = match KVStore::open(file.path()) {
            Ok(_) => panic!("expected malformed log to fail"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn encodes_header() {
        let header = KVStore::encode_header(SET_TAG, 3, 4);

        assert_eq!(header, [0x01, 0x00, 0x03, 0x00, 0x04])
    }
}
