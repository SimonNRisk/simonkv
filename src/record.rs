use crc32fast::Hasher;
use std::io::{self, Read};

pub(crate) const HEADER_LEN: usize = 1 + 2 + 4;
pub(crate) const CHECKSUM_LEN: usize = 4;
pub(crate) const SET_TAG: u8 = 0x01;
pub(crate) const DELETE_TAG: u8 = 0x02;

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Command {
    Set(String, String),
    Delete(String),
}

pub(crate) struct DecodedRecord {
    pub(crate) command: Command,
    pub(crate) encoded_len: u64,
}

pub(crate) fn encode_set(key: &str, value: &str) -> io::Result<Vec<u8>> {
    encode_record(SET_TAG, key, value.as_bytes())
}

pub(crate) fn encode_delete(key: &str) -> io::Result<Vec<u8>> {
    encode_record(DELETE_TAG, key, &[])
}

pub(crate) fn decode(reader: &mut impl Read) -> io::Result<Option<DecodedRecord>> {
    let mut header = [0u8; HEADER_LEN];

    // A clean EOF before a new record is normal. Once the first byte exists,
    // however, the rest of the record must also exist.
    match reader.read_exact(&mut header[..1]) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }

    read_exact_or_truncated(reader, &mut header[1..], "truncated log header")?;

    let operation = header[0];
    if !matches!(operation, SET_TAG | DELETE_TAG) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unknown log operation",
        ));
    }

    let mut stored_header_checksum = [0u8; CHECKSUM_LEN];
    read_exact_or_truncated(
        reader,
        &mut stored_header_checksum,
        "truncated log header checksum",
    )?;

    let stored_header_checksum = u32::from_be_bytes(stored_header_checksum);
    let computed_header_checksum = checksum(&header, &[]);
    if stored_header_checksum != computed_header_checksum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "record header checksum mismatch",
        ));
    }

    let key_len = u16::from_be_bytes([header[1], header[2]]) as usize;
    let value_len = u32::from_be_bytes([header[3], header[4], header[5], header[6]]) as usize;
    let payload_len = key_len.checked_add(value_len).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "record payload length overflow")
    })?;

    let mut payload = vec![0u8; payload_len];
    read_exact_or_truncated(reader, &mut payload, "truncated log payload")?;

    let mut stored_checksum = [0u8; CHECKSUM_LEN];
    read_exact_or_truncated(reader, &mut stored_checksum, "truncated log checksum")?;

    let stored_checksum = u32::from_be_bytes(stored_checksum);
    let computed_checksum = checksum(&header, &payload);
    if stored_checksum != computed_checksum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "record checksum mismatch",
        ));
    }

    let key = String::from_utf8(payload[..key_len].to_vec()).map_err(|_error| {
        io::Error::new(io::ErrorKind::InvalidData, "log key is not valid UTF-8")
    })?;
    let value = String::from_utf8(payload[key_len..].to_vec()).map_err(|_error| {
        io::Error::new(io::ErrorKind::InvalidData, "log value is not valid UTF-8")
    })?;
    let encoded_len = (HEADER_LEN + 2 * CHECKSUM_LEN) as u64 + key_len as u64 + value_len as u64;

    let command = match operation {
        SET_TAG => Command::Set(key, value),
        DELETE_TAG => Command::Delete(key),
        _ => unreachable!("operation was validated above"),
    };

    Ok(Some(DecodedRecord {
        command,
        encoded_len,
    }))
}

pub(crate) fn encode_header(operation: u8, key_len: u16, value_len: u32) -> [u8; HEADER_LEN] {
    let mut header = [0u8; HEADER_LEN];
    header[0] = operation;
    header[1..3].copy_from_slice(&key_len.to_be_bytes());
    header[3..7].copy_from_slice(&value_len.to_be_bytes());
    header
}

pub(crate) fn checksum(header: &[u8], payload: &[u8]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(header);
    hasher.update(payload);
    hasher.finalize()
}

fn encode_record(operation: u8, key: &str, value_bytes: &[u8]) -> io::Result<Vec<u8>> {
    if !matches!(operation, SET_TAG | DELETE_TAG) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported operation",
        ));
    }

    let key_bytes = key.as_bytes();
    let key_len = u16::try_from(key_bytes.len())
        .map_err(|_error| io::Error::new(io::ErrorKind::InvalidInput, "key too large"))?;
    let value_len = u32::try_from(value_bytes.len())
        .map_err(|_error| io::Error::new(io::ErrorKind::InvalidInput, "value too large"))?;

    let header = encode_header(operation, key_len, value_len);
    let header_checksum = checksum(&header, &[]);
    let mut result =
        Vec::with_capacity(header.len() + key_bytes.len() + value_bytes.len() + 2 * CHECKSUM_LEN);

    result.extend_from_slice(&header);
    result.extend_from_slice(&header_checksum.to_be_bytes());
    let payload_start = result.len();
    result.extend_from_slice(key_bytes);
    result.extend_from_slice(value_bytes);
    let record_checksum = checksum(&header, &result[payload_start..]);
    result.extend_from_slice(&record_checksum.to_be_bytes());

    Ok(result)
}

fn read_exact_or_truncated(
    reader: &mut impl Read,
    buffer: &mut [u8],
    message: &'static str,
) -> io::Result<()> {
    reader.read_exact(buffer).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            io::Error::new(io::ErrorKind::UnexpectedEof, message)
        } else {
            error
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn set_roundtrip_includes_encoded_length() {
        let encoded = encode_set("key", "value").unwrap();
        let decoded = decode(&mut Cursor::new(&encoded)).unwrap().unwrap();

        assert_eq!(decoded.command, Command::Set("key".into(), "value".into()));
        assert_eq!(decoded.encoded_len, encoded.len() as u64);
    }

    #[test]
    fn delete_roundtrip_includes_encoded_length() {
        let encoded = encode_delete("key").unwrap();
        let decoded = decode(&mut Cursor::new(&encoded)).unwrap().unwrap();

        assert_eq!(decoded.command, Command::Delete("key".into()));
        assert_eq!(decoded.encoded_len, encoded.len() as u64);
    }

    #[test]
    fn clean_end_of_input_has_no_record() {
        assert!(decode(&mut Cursor::new([])).unwrap().is_none());
    }

    #[test]
    fn encodes_header() {
        let header = encode_header(SET_TAG, 3, 4);

        assert_eq!(header, [0x01, 0x00, 0x03, 0x00, 0x00, 0x00, 0x04]);
    }
}
