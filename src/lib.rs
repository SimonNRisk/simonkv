use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

pub struct KVStore {
    map: HashMap<String, String>,
    log: File,
}

enum Command {
    Set(String, String),
    Delete(String),
}

const HEADER_LEN: usize = 1 + 2 + 4;
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
        let mut reader = &log;

        loop {
            match Self::read_record(&mut reader)? {
                Some(command) => Self::apply_command(&mut map, command),
                None => break,
            }
        }
        Ok(KVStore { map, log })
    }
    pub fn set(&mut self, key: String, value: String) -> io::Result<()> {
        let record = Self::encode_set(&key, &value)?;
        self.log.write_all(&record)?;
        self.map.insert(key, value);
        Ok(())
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(|v| v.as_str())
    }
    pub fn delete(&mut self, key: &str) -> io::Result<Option<String>> {
        let record = Self::encode_delete(key)?;
        self.log.write_all(&record)?;
        Ok(self.map.remove(key))
    }
    fn read_record(reader: &mut impl Read) -> io::Result<Option<Command>> {
        let mut header = [0u8; HEADER_LEN];

        // A clean EOF before a new record is normal. Once the first byte
        // exists, however, the rest of the header must also exist.
        match reader.read_exact(&mut header[..1]) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        }

        // At this point, we know we have content to read
        reader.read_exact(&mut header[1..]).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                io::Error::new(io::ErrorKind::InvalidData, "truncated log header")
            } else {
                error
            }
        })?;

        let operation = header[0];
        let key_len = u16::from_be_bytes([header[1], header[2]]) as usize;
        let value_len = u32::from_be_bytes([header[3], header[4], header[5], header[6]]) as usize;

        let mut payload = vec![0u8; key_len + value_len];
        reader.read_exact(&mut payload).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                io::Error::new(io::ErrorKind::InvalidData, "truncated log payload")
            } else {
                error
            }
        })?;

        let key = String::from_utf8(payload[..key_len].to_vec()).map_err(|_error| {
            io::Error::new(io::ErrorKind::InvalidData, "log key is not valid UTF-8")
        })?;
        let value = String::from_utf8(payload[key_len..].to_vec()).map_err(|_error| {
            io::Error::new(io::ErrorKind::InvalidData, "log value is not valid UTF-8")
        })?;

        match operation {
            SET_TAG => Ok(Some(Command::Set(key, value))),
            DELETE_TAG => Ok(Some(Command::Delete(key))),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown log operation",
            )),
        }
    }

    fn apply_command(map: &mut HashMap<String, String>, command: Command) {
        match command {
            Command::Set(key, value) => {
                map.insert(key, value);
            }
            Command::Delete(key) => {
                map.remove(&key);
            }
        }
    }
    fn encode_header(operation: u8, key_len: u16, value_len: u32) -> [u8; HEADER_LEN] {
        let mut header = [0u8; HEADER_LEN];

        header[0] = operation;

        header[1..3].copy_from_slice(&key_len.to_be_bytes());
        header[3..7].copy_from_slice(&value_len.to_be_bytes());

        header
    }
    fn encode_set(key: &str, value: &str) -> io::Result<Vec<u8>> {
        Self::encode_record(SET_TAG, key, value.as_bytes())
    }

    fn encode_delete(key: &str) -> io::Result<Vec<u8>> {
        Self::encode_record(DELETE_TAG, key, &[])
    }

    fn encode_record(operation: u8, key: &str, value_bytes: &[u8]) -> io::Result<Vec<u8>> {
        if !matches!(operation, SET_TAG | DELETE_TAG) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unsupported operation",
            ));
        }

        let key_bytes = key.as_bytes();
        let key_len = u16::try_from(key_bytes.len())
            .map_err(|_err| io::Error::new(io::ErrorKind::InvalidInput, "Key too large"))?;

        let value_len = u32::try_from(value_bytes.len())
            .map_err(|_err| io::Error::new(io::ErrorKind::InvalidInput, "Value too large"))?;

        let header = KVStore::encode_header(operation, key_len, value_len);

        let mut result: Vec<u8> =
            Vec::with_capacity(header.len() + key_bytes.len() + value_bytes.len());

        result.extend_from_slice(&header);
        result.extend_from_slice(key_bytes);
        result.extend_from_slice(value_bytes);

        Ok(result)
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

        let contents = std::fs::read(file.path()).unwrap();
        assert_eq!(
            contents,
            vec![0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, b'K', b'V']
        );
    }

    #[test]
    fn del_writes_to_log() {
        let (mut store, file) = make_store();

        store.delete("K".into()).unwrap();

        drop(store);

        let contents = std::fs::read(file.path()).unwrap();
        assert_eq!(
            contents,
            vec![0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, b'K']
        );
    }

    #[test]
    fn set_del_roundtrip_writes_to_log() {
        let (mut store, file) = make_store();

        store.set("K".into(), "V".into()).unwrap();
        store.delete("K").unwrap();

        drop(store);

        let contents = std::fs::read(file.path()).unwrap();
        assert_eq!(
            contents,
            vec![
                0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, b'K', b'V', 0x02, 0x00, 0x01, 0x00, 0x00,
                0x00, 0x00, b'K'
            ]
        );
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

    fn assert_open_rejects(bytes: &[u8]) {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(file.path(), bytes).unwrap();

        let error = match KVStore::open(file.path()) {
            Ok(_) => panic!("expected malformed log to fail"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn unknown_operation_returns_error() {
        assert_open_rejects(&[0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn truncated_header_returns_error() {
        assert_open_rejects(&[SET_TAG, 0x00]);
    }

    #[test]
    fn truncated_payload_returns_error() {
        assert_open_rejects(&[SET_TAG, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn invalid_utf8_returns_error() {
        assert_open_rejects(&[SET_TAG, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn valid_record_followed_by_truncated_record_returns_error() {
        let mut bytes = KVStore::encode_set("K1", "V1").unwrap();
        bytes.extend_from_slice(&[SET_TAG, 0x00]);
        assert_open_rejects(&bytes);
    }

    #[test]
    fn encodes_header() {
        let header = KVStore::encode_header(SET_TAG, 3, 4);

        assert_eq!(header, [0x01, 0x00, 0x03, 0x00, 0x00, 0x00, 0x04])
    }

    #[test]
    fn handles_whitespace_in_key_value() {
        let key = "   whitespace.   ";
        let value = "    more white space   ";

        let (mut store, file) = make_store();

        store.set(key.to_string(), value.to_string()).unwrap();

        drop(store);

        let store = KVStore::open(file).unwrap();

        assert_eq!(store.get(key), Some(value));
    }

    #[test]
    fn handles_newlines_in_key_value() {
        let key = "\n new linen\n\n";
        let value = "\n\n\n\n newer line \n";

        let (mut store, file) = make_store();

        store.set(key.to_string(), value.to_string()).unwrap();

        drop(store);

        let store = KVStore::open(file).unwrap();

        assert_eq!(store.get(key), Some(value));
    }
}
