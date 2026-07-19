use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub struct KVStore {
    keydir: HashMap<String, Location>,
    log: File,
    path: PathBuf,
}

enum Command {
    Set(String, String),
    Delete(String),
}

struct Location {
    offset: u64,
}

struct DecodedRecord {
    command: Command,
    length: u64,
}

const HEADER_LEN: usize = 1 + 2 + 4;
const SET_TAG: u8 = 0x01; // Just a hex representation for 1
const DELETE_TAG: u8 = 0x02; // Just a hex representation for 2

impl KVStore {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;

        let mut keydir = HashMap::new();
        let mut reader = &log;
        let mut offset = 0u64;

        loop {
            match Self::read_record(&mut reader)? {
                Some(record) => {
                    match record.command {
                        Command::Set(key, _) => {
                            keydir.insert(key, Location { offset });
                        }
                        Command::Delete(key) => {
                            keydir.remove(&key);
                        }
                    }
                    offset += record.length;
                }
                None => break,
            }
        }
        Ok(KVStore { keydir, log, path })
    }
    pub fn set(&mut self, key: String, value: String) -> io::Result<()> {
        let record = Self::encode_set(&key, &value)?;
        let offset = self.log.metadata()?.len();
        self.log.write_all(&record)?;
        self.keydir.insert(key, Location { offset });
        Ok(())
    }
    pub fn get(&mut self, key: &str) -> io::Result<Option<String>> {
        let Some(location) = self.keydir.get(key) else {
            return Ok(None);
        };

        self.log.seek(SeekFrom::Start(location.offset))?;

        let Some(record) = Self::read_record(&mut self.log)? else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "keydir points past the end of the log",
            ));
        };

        match record.command {
            Command::Set(record_key, value) if record_key == key => Ok(Some(value)),
            Command::Set(_, _) | Command::Delete(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "keydir points to the wrong record",
            )),
        }
    }
    pub fn delete(&mut self, key: &str) -> io::Result<Option<String>> {
        let old_value = self.get(key)?;
        let record = Self::encode_delete(key)?;
        self.log.write_all(&record)?;
        self.keydir.remove(key);
        Ok(old_value)
    }

    pub fn compact(&mut self) -> io::Result<()> {
        let mut compact_path = self.path.as_os_str().to_os_string();
        compact_path.push(".compact");
        let compact_path = PathBuf::from(compact_path);

        let mut compacted_log = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&compact_path)?;

        let keys: Vec<String> = self.keydir.keys().cloned().collect();

        for key in keys {
            let value = self.get(&key)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "keydir contains a missing key")
            })?;
            let record = Self::encode_set(&key, &value)?; // DELs wont be in keydir.keys()
            compacted_log.write_all(&record)?;
        }
        compacted_log.sync_all()?;
        drop(compacted_log);

        std::fs::rename(&compact_path, &self.path)?;

        // Recreates kedir and updates self.log handle
        let reopened_store = Self::open(&self.path)?;
        *self = reopened_store;

        Ok(())
    }

    fn read_record(reader: &mut impl Read) -> io::Result<Option<DecodedRecord>> {
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
            SET_TAG => Ok(Some(DecodedRecord {
                command: Command::Set(key, value),
                length: (HEADER_LEN + key_len + value_len) as u64,
            })),
            DELETE_TAG => Ok(Some(DecodedRecord {
                command: Command::Delete(key),
                length: (HEADER_LEN + key_len + value_len) as u64,
            })),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown log operation",
            )),
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
        assert_eq!(store.get(key).unwrap(), Some(val.to_string()));
    }

    #[test]
    fn get_empty_value_returns_none() {
        let (mut store, _) = make_store();
        assert_eq!(store.get("K").unwrap(), None);
    }

    #[test]
    fn get_empty_value_is_distinct_from_missing() {
        let (mut store, _) = make_store();
        store.set("K".into(), "".into()).unwrap();
        assert_eq!(store.get("K").unwrap(), Some(String::new()));
        assert_eq!(store.get("absent").unwrap(), None);
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
        assert_eq!(store.get(key).unwrap(), None);
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

        let mut store = KVStore::open(file.path()).unwrap();
        assert_eq!(store.get("K").unwrap(), Some("V".to_string()));
    }

    #[test]
    fn open_stores_most_recent_set_from_log() {
        let (mut store, file) = make_store();

        store.set("K".into(), "V".into()).unwrap();
        store.set("K".into(), "V2".into()).unwrap();

        drop(store);

        let mut store = KVStore::open(file.path()).unwrap();
        assert_eq!(store.get("K").unwrap(), Some("V2".to_string()));
    }

    #[test]
    fn open_restores_most_recent_data_set_delete() {
        let (mut store, file) = make_store();

        store.set("K".into(), "V".into()).unwrap();
        store.set("K2".into(), "V2".into()).unwrap();

        store.delete("K").unwrap();

        drop(store);

        let mut store = KVStore::open(file.path()).unwrap();
        assert_eq!(store.get("K").unwrap(), None);
        assert_eq!(store.get("K2").unwrap(), Some("V2".to_string()));
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

        let mut store = KVStore::open(file).unwrap();

        assert_eq!(store.get(key).unwrap(), Some(value.to_string()));
    }

    #[test]
    fn handles_newlines_in_key_value() {
        let key = "\n new linen\n\n";
        let value = "\n\n\n\n newer line \n";

        let (mut store, file) = make_store();

        store.set(key.to_string(), value.to_string()).unwrap();

        drop(store);

        let mut store = KVStore::open(file).unwrap();

        assert_eq!(store.get(key).unwrap(), Some(value.to_string()));
    }

    #[test]
    fn compact_writes_only_latest_live_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.log");
        let mut store = KVStore::open(&path).unwrap();

        store.set("K".into(), "old".into()).unwrap();
        store.set("K".into(), "new".into()).unwrap();

        store.set("deleted".into(), "value".into()).unwrap();
        store.delete("deleted").unwrap();

        store.compact().unwrap();

        let mut compact_path = path.as_os_str().to_os_string();
        compact_path.push(".compact");
        let compact_path = PathBuf::from(compact_path);
        let actual = std::fs::read(&path).unwrap();
        let expected = KVStore::encode_set("K", "new").unwrap();

        assert_eq!(actual, expected);
        assert!(!compact_path.exists());
        assert_eq!(store.get("K").unwrap(), Some("new".into()));
        assert_eq!(store.get("deleted").unwrap(), None);
    }

    #[test]
    fn compacted_store_accepts_new_writes_and_reopens() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.log");
        let mut store = KVStore::open(&path).unwrap();

        store.set("before".into(), "compaction".into()).unwrap();
        store.compact().unwrap();
        store.set("after".into(), "compaction".into()).unwrap();

        drop(store);

        let mut reopened_store = KVStore::open(&path).unwrap();
        assert_eq!(
            reopened_store.get("before").unwrap(),
            Some("compaction".into())
        );
        assert_eq!(
            reopened_store.get("after").unwrap(),
            Some("compaction".into())
        );
    }

    #[test]
    fn store_ending_in_compact_survives() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.compact");
        let mut store = KVStore::open(&path).unwrap();

        store.set("key".into(), "value".into()).unwrap();
        store.set("key".into(), "better value".into()).unwrap();

        store.compact().unwrap();

        assert_eq!(store.get("key").unwrap(), Some("better value".into()));
    }
}
