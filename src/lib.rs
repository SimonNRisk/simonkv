mod record;
mod scanner;

use record::Command;
use scanner::RecordScanner;
use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub struct KVStore {
    keydir: HashMap<String, Location>,
    log: File,
    path: PathBuf,
}

struct Location {
    segment_id: u64,
    offset: u64,
    record_length: u64,
}

const DEFAULT_SEGMENT_LENGTH: u64 = 64 * 1024 * 1024; // 64MiB

pub struct StoreOptions {
    segment_length: u64,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self {
            segment_length: DEFAULT_SEGMENT_LENGTH
        }
    }
}

impl StoreOptions {
    pub fn segment_length(mut self, bytes: u64) {
        self.segment_length = bytes;
        self
    }

    fn validate(self) -> io::Result<()> {
        if self.segment_length == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Segment length must be greater than zero"))
        }
        Ok(())
    }
}

impl KVStore {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;

        Self::from_open_log(path, log)
    }

    fn from_open_log(path: PathBuf, mut log: File) -> io::Result<Self> {
        log.seek(SeekFrom::Start(0))?;

        let mut keydir = HashMap::new();
        let torn_tail_start = {
            let mut scanner = RecordScanner::new(&mut log);
            let mut last_valid_end = 0;

            loop {
                let record = match scanner.next() {
                    Some(Ok(record)) => record,
                    None => break None,
                    Some(Err(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                        break Some(last_valid_end);
                    }
                    Some(Err(error)) => return Err(error),
                };

                last_valid_end = record.offset + record.record_len;
                match record.command {
                    Command::Set(key, _) => {
                        keydir.insert(
                            key,
                            Location {
                                offset: record.offset,
                            },
                        );
                    }
                    Command::Delete(key) => {
                        keydir.remove(&key);
                    }
                }
            }
        };

        if let Some(torn_tail_start) = torn_tail_start {
            // Remove the torn tail so future writes follow the last valid record.
            log.set_len(torn_tail_start)?;
            log.sync_data()?;
        }

        Ok(KVStore { keydir, log, path })
    }

    pub fn set(&mut self, key: String, value: String) -> io::Result<()> {
        let record = record::encode_set(&key, &value)?;
        let offset = self.log.metadata()?.len();
        self.log.write_all(&record)?;
        self.log.sync_data()?;
        self.keydir.insert(key, Location { offset });
        Ok(())
    }
    pub fn get(&mut self, key: &str) -> io::Result<Option<String>> {
        let Some(location) = self.keydir.get(key) else {
            return Ok(None);
        };

        self.log.seek(SeekFrom::Start(location.offset))?;

        let Some(record) = record::decode(&mut self.log)? else {
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
        let record = record::encode_delete(key)?;
        self.log.write_all(&record)?;
        self.log.sync_data()?;
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
            let record = record::encode_set(&key, &value)?; // DELs wont be in keydir.keys()
            compacted_log.write_all(&record)?;
        }
        compacted_log.sync_all()?;

        // Some OS require file to be closed to rename
        drop(compacted_log);

        std::fs::rename(&compact_path, &self.path)?;

        // Persist the directory entry changed by rename before accepting new writes
        let parent_directory = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        File::open(parent_directory)?.sync_all()?;

        // Recreates kedir and updates self.log handle
        let reopened_store = Self::open(&self.path)?;
        *self = reopened_store;

        Ok(())
    }
}

impl fmt::Display for KVStore {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut first = true;
        for (key, location) in &self.keydir {
            if !first {
                write!(f, ", ")?;
            }
            let offset = location.offset;
            write!(f, "{{{key}: {offset}}}")?;
            first = false
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{
        DELETE_TAG, HEADER_LEN, SET_TAG, checksum, encode_delete, encode_header, encode_set,
    };
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
        assert_eq!(contents, encode_set("K", "V").unwrap());
    }

    #[test]
    fn del_writes_to_log() {
        let (mut store, file) = make_store();

        store.delete("K").unwrap();

        drop(store);

        let contents = std::fs::read(file.path()).unwrap();
        assert_eq!(contents, encode_delete("K").unwrap());
    }

    #[test]
    fn set_del_roundtrip_writes_to_log() {
        let (mut store, file) = make_store();

        store.set("K".into(), "V".into()).unwrap();
        store.delete("K").unwrap();

        drop(store);

        let contents = std::fs::read(file.path()).unwrap();
        let mut expected = encode_set("K", "V").unwrap();
        expected.extend_from_slice(&encode_delete("K").unwrap());
        assert_eq!(contents, expected);
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
    fn open_replays_from_start_when_file_cursor_is_at_end() {
        let file = NamedTempFile::new().unwrap();
        let record = encode_set("K", "V").unwrap();
        std::fs::write(file.path(), record).unwrap();

        let mut log = OpenOptions::new()
            .append(true)
            .read(true)
            .open(file.path())
            .unwrap();
        log.seek(SeekFrom::End(0)).unwrap();

        let mut store = KVStore::from_open_log(file.path().to_path_buf(), log).unwrap();

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

    fn assert_truncated_log_is_recovered_as_empty(bytes: &[u8]) {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(file.path(), bytes).unwrap();

        let store = KVStore::open(file.path()).unwrap();

        assert!(store.keydir.is_empty());
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), 0);
    }

    #[test]
    fn unknown_operation_returns_error() {
        assert_open_rejects(&[0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn truncated_header_is_discarded() {
        assert_truncated_log_is_recovered_as_empty(&[SET_TAG, 0x00]);
    }

    #[test]
    fn truncated_payload_is_discarded() {
        assert_truncated_log_is_recovered_as_empty(&[SET_TAG, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn invalid_utf8_returns_error() {
        let header = encode_header(SET_TAG, 1, 0);
        let payload = [0xff];
        let mut record = Vec::from(header);
        record.extend_from_slice(&checksum(&header, &[]).to_be_bytes());
        record.extend_from_slice(&payload);
        record.extend_from_slice(&checksum(&header, &payload).to_be_bytes());

        assert_open_rejects(&record);
    }

    #[test]
    fn payload_corruption_returns_error() {
        let mut record = encode_set("K", "V").unwrap();
        record[HEADER_LEN + 1] = b'X';

        assert_open_rejects(&record);
    }

    #[test]
    fn operation_corruption_returns_error() {
        let mut record = encode_set("K", "V").unwrap();
        record[0] = DELETE_TAG;

        assert_open_rejects(&record);
    }

    #[test]
    fn length_corruption_returns_error_without_truncating_log() {
        let file = NamedTempFile::new().unwrap();
        let mut record = encode_set("K", "V").unwrap();
        record[6] = 0x02;
        std::fs::write(file.path(), &record).unwrap();

        let result = KVStore::open(file.path());

        assert!(matches!(
            result,
            Err(ref error) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert_eq!(std::fs::read(file.path()).unwrap(), record);
    }

    #[test]
    fn valid_records_before_truncated_tail_are_recovered() {
        let file = NamedTempFile::new().unwrap();
        let mut bytes = encode_set("K1", "V1").unwrap();
        let valid_length = bytes.len() as u64;
        let second_record = encode_set("K2", "V2").unwrap();
        bytes.extend_from_slice(&second_record[..second_record.len() - 1]);
        std::fs::write(file.path(), bytes).unwrap();

        let mut store = KVStore::open(file.path()).unwrap();

        assert_eq!(store.get("K1").unwrap(), Some("V1".to_string()));
        assert_eq!(store.get("K2").unwrap(), None);
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), valid_length);
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
        let expected = encode_set("K", "new").unwrap();

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
