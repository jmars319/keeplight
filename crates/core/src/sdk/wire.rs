//! The OpenRGB SDK's framing and field encoding.
//!
//! Every packet starts with a 16-byte header: the magic `ORGB`, then three
//! little-endian `u32`s for the device index, the packet id and the payload
//! size. Strings are a `u16` length that counts a trailing NUL, then the bytes.

use super::SdkError;
use crate::device::{Mode, Rgb};

pub const MAGIC: &[u8; 4] = b"ORGB";
pub const HEADER_LEN: usize = 16;

/// Packet ids Keeplight sends or expects. Of the requests that change lights,
/// only the two that set one device's mode and colors are here; profile
/// loading and saving to a device's own memory are left out on purpose
/// (`docs/safety-model.md`).
pub mod packet {
    pub const REQUEST_CONTROLLER_COUNT: u32 = 0;
    pub const REQUEST_CONTROLLER_DATA: u32 = 1;
    pub const REQUEST_PROTOCOL_VERSION: u32 = 40;
    pub const SET_CLIENT_NAME: u32 = 50;
    pub const DEVICE_LIST_UPDATED: u32 = 100;
    /// Sets every light on one device; no reply.
    pub const UPDATE_LEDS: u32 = 1050;
    /// Switches one device to a mode with the given settings; no reply.
    pub const UPDATE_MODE: u32 = 1101;
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

/// Builds a little-endian payload; the counterpart of [`Reader`].
#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// A `u16` length counting the trailing NUL, then the bytes and the NUL.
    pub fn string(&mut self, text: &str) -> &mut Self {
        self.u16((text.len() + 1) as u16);
        self.bytes.extend_from_slice(text.as_bytes());
        self.bytes.push(0);
        self
    }

    pub fn colors(&mut self, colors: &[Rgb]) -> &mut Self {
        self.u16(colors.len() as u16);
        for c in colors {
            self.u32(c.to_wire());
        }
        self
    }

    pub fn mode(&mut self, m: &Mode, protocol: u32) -> &mut Self {
        self.string(&m.name).i32(m.value).u32(m.flags).u32(m.speed_min).u32(m.speed_max);
        if protocol >= 3 {
            self.u32(m.brightness_min).u32(m.brightness_max);
        }
        self.u32(m.colors_min).u32(m.colors_max).u32(m.speed);
        if protocol >= 3 {
            self.u32(m.brightness);
        }
        self.u32(m.direction).u32(m.color_mode).colors(&m.colors)
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The bytes with a leading `u32` total size that counts itself, as the
    /// `UPDATE_LEDS` and `UPDATE_MODE` payloads start.
    pub fn into_sized(self) -> Vec<u8> {
        let size = (self.bytes.len() + 4) as u32;
        let mut out = size.to_le_bytes().to_vec();
        out.extend(self.bytes);
        out
    }
}

/// `UPDATE_LEDS`: one color for every light on the device, in order.
pub fn update_leds(colors: &[Rgb]) -> Vec<u8> {
    let mut w = Writer::default();
    w.colors(colors);
    w.into_sized()
}

/// `UPDATE_MODE`: the mode's index on the device, then the mode itself.
pub fn update_mode(index: usize, mode: &Mode, protocol: u32) -> Vec<u8> {
    let mut w = Writer::default();
    w.i32(index as i32).mode(mode, protocol);
    w.into_sized()
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
    fn update_leds_counts_its_own_size() {
        let red = Rgb { r: 255, g: 0, b: 0 };
        let bytes = update_leds(&[red, red]);
        let mut r = Reader::new(&bytes);
        assert_eq!(r.u32().unwrap() as usize, bytes.len());
        assert_eq!(r.u16().unwrap(), 2);
        assert_eq!(r.u32().unwrap(), 0x0000ff);
    }

    #[test]
    fn update_mode_carries_the_index_then_the_mode() {
        let mode = Mode { name: "Static".into(), value: 1, color_mode: 2, ..Mode::default() };
        let bytes = update_mode(4, &mode, 3);
        let mut r = Reader::new(&bytes);
        assert_eq!(r.u32().unwrap() as usize, bytes.len());
        assert_eq!(r.i32().unwrap(), 4);
        assert_eq!(r.string().unwrap(), "Static");
        // name, value, flags, 10 words at protocol 3, then no colors.
        assert_eq!(bytes.len(), 4 + 4 + (2 + 7) + 4 + 4 + 40 + 2);
        assert_eq!(update_mode(4, &mode, 2).len(), bytes.len() - 12);
    }

    #[test]
    fn short_payloads_fail_instead_of_panicking() {
        let mut r = Reader::new(&[1, 0]);
        assert!(r.u32().is_err());
        let mut r = Reader::new(&[9, 0, b'a']);
        assert!(r.string().is_err());
    }
}
