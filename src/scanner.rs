use crate::record::{self, Command};
use std::io::{self, Read};

#[derive(Debug)]
pub(crate) struct ScannedRecord {
    pub(crate) command: Command,
    pub(crate) offset: u64,
    pub(crate) record_len: u64,
}

pub(crate) struct RecordScanner<R: Read> {
    reader: R,
    offset: u64,
    finished: bool,
}

impl<R: Read> RecordScanner<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader,
            offset: 0,
            finished: false,
        }
    }
}

impl<R: Read> Iterator for RecordScanner<R> {
    type Item = io::Result<ScannedRecord>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        let decoded = match record::decode(&mut self.reader) {
            Ok(Some(record)) => record,
            Ok(None) => {
                self.finished = true;
                return None;
            }
            Err(error) => {
                self.finished = true;
                return Some(Err(error));
            }
        };

        let next_offset = match self.offset.checked_add(decoded.encoded_len) {
            Some(offset) => offset,
            None => {
                self.finished = true;
                return Some(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "record offset overflow",
                )));
            }
        };
        let record = ScannedRecord {
            command: decoded.command,
            offset: self.offset,
            record_len: decoded.encoded_len,
        };

        self.offset = next_offset;
        Some(Ok(record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Command, encode_delete, encode_set};
    use std::io::Cursor;

    #[test]
    fn scans_records_with_offsets_and_lengths() {
        let first = encode_set("key", "value").unwrap();
        let second = encode_delete("another-key").unwrap();
        let mut bytes = first.clone();
        bytes.extend_from_slice(&second);

        let mut scanner = RecordScanner::new(Cursor::new(bytes));
        let first_record = scanner.next().unwrap().unwrap();
        let second_record = scanner.next().unwrap().unwrap();

        assert_eq!(
            first_record.command,
            Command::Set("key".into(), "value".into())
        );
        assert_eq!(first_record.offset, 0);
        assert_eq!(first_record.record_len, first.len() as u64);
        assert_eq!(second_record.command, Command::Delete("another-key".into()));
        assert_eq!(second_record.offset, first.len() as u64);
        assert_eq!(second_record.record_len, second.len() as u64);
        assert_eq!(scanner.offset, (first.len() + second.len()) as u64);
        assert!(scanner.next().is_none());
    }

    #[test]
    fn offset_stops_after_last_complete_record() {
        let first = encode_set("key", "value").unwrap();
        let second = encode_delete("another-key").unwrap();
        let mut bytes = first.clone();
        bytes.extend_from_slice(&second[..second.len() - 1]);

        let mut scanner = RecordScanner::new(Cursor::new(bytes));

        scanner.next().unwrap().unwrap();
        let error = scanner.next().unwrap().unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(scanner.offset, first.len() as u64);
        assert!(scanner.next().is_none());
    }
}
