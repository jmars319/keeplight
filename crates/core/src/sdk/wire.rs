//! The OpenRGB SDK's framing and field encoding.
//!
//! Every packet starts with a 16-byte header: the magic `ORGB`, then three
//! little-endian `u32`s for the device index, the packet id and the payload
//! size. Strings are a `u16` length that counts a trailing NUL, then the bytes.

use super::SdkError;

pub const MAGIC: &[u8; 4] = b"ORGB";
pub const HEADER_LEN: usize = 16;

/// Packet ids Keeplight sends or expects. Ids that change lights are left out
/// on purpose until a stage needs them.
pub mod packet {
    pub const REQUEST_CONTROLLER_COUNT: u32 = 0;
    pub const REQUEST_CONTROLLER_DATA: u32 = 1;
    pub const REQUEST_PROTOCOL_VERSION: u32 = 40;
    pub const SET_CLIENT_NAME: u32 = 50;
    pub const DEVICE_LIST_UPDATED: u32 = 100;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub device: u32,
    pub id: u32,
    pub size: u32,
}

pub fn encode(device: u32, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&device.to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

pub fn decode_header(bytes: &[u8; HEADER_LEN]) -> Result<Header, SdkError> {
    if &bytes[..4] != MAGIC {
        return Err(SdkError::Protocol("a reply did not start with ORGB".into()));
    }
    let word = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    Ok(Header { device: word(4), id: word(8), size: word(12) })
}

/// A null-terminated string as the SDK sends it in `SET_CLIENT_NAME`.
pub fn c_string(text: &str) -> Vec<u8> {
    let mut out = text.as_bytes().to_vec();
    out.push(0);
    out
}

/// Reads little-endian fields from a payload, failing cleanly when it ends
/// early instead of panicking on a short or garbled reply.
pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], SdkError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.bytes.len()).ok_or_else(|| {
            SdkError::Protocol(format!("a reply ended early (wanted {n} bytes at offset {})", self.pos))
        })?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn u16(&mut self) -> Result<u16, SdkError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, SdkError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32, SdkError> {
        Ok(self.u32()? as i32)
    }

    pub fn string(&mut self) -> Result<String, SdkError> {
        let len = self.u16()? as usize;
        let raw = self.take(len)?;
        let text = raw.strip_suffix(&[0]).unwrap_or(raw);
        Ok(String::from_utf8_lossy(text).into_owned())
    }

    pub fn skip(&mut self, n: usize) -> Result<(), SdkError> {
        self.take(n).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let bytes = encode(3, packet::REQUEST_CONTROLLER_DATA, &[1, 2, 3]);
        assert_eq!(bytes.len(), HEADER_LEN + 3);
        let head: [u8; HEADER_LEN] = bytes[..HEADER_LEN].try_into().unwrap();
        assert_eq!(decode_header(&head).unwrap(), Header { device: 3, id: 1, size: 3 });
    }

    #[test]
    fn bad_magic_is_an_error() {
        let mut head = [0u8; HEADER_LEN];
        head[..4].copy_from_slice(b"HTTP");
        assert!(decode_header(&head).is_err());
    }

    #[test]
    fn strings_drop_their_trailing_nul() {
        let mut bytes = 4u16.to_le_bytes().to_vec();
        bytes.extend_from_slice(b"RAM\0");
        assert_eq!(Reader::new(&bytes).string().unwrap(), "RAM");
    }

    #[test]
    fn short_payloads_fail_instead_of_panicking() {
        let mut r = Reader::new(&[1, 0]);
        assert!(r.u32().is_err());
        let mut r = Reader::new(&[9, 0, b'a']);
        assert!(r.string().is_err());
    }
}
