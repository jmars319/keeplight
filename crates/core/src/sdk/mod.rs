//! A synchronous client for OpenRGB's SDK server.
//!
//! Written from OpenRGB's protocol description rather than taken from an
//! existing crate, because the maintained one is GPL-2.0-only and this app is
//! GPL-3.0-only (`docs/adr/0001-build-on-openrgb.md`).

pub mod wire;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::device::{Device, Mode, Rgb};
use wire::{HEADER_LEN, packet};

/// Where OpenRGB listens unless told otherwise.
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:6742";

/// The highest protocol version this client can decode. The server answers
/// with its own; both sides then use the lower one.
pub const CLIENT_PROTOCOL: u32 = 3;

/// How this client shows up in OpenRGB's list of connected clients.
pub const CLIENT_NAME: &str = "Keeplight";

const CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// A reply larger than this is treated as garbage rather than allocated.
const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("OpenRGB isn't answering at {address}")]
    Unreachable { address: String, source: std::io::Error },
    #[error("the connection to OpenRGB failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("OpenRGB sent something Keeplight could not read: {0}")]
    Protocol(String),
}

/// Everything OpenRGB reported in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The server's own protocol version, before negotiation.
    pub server_protocol: u32,
    pub devices: Vec<Device>,
}

pub struct Client {
    stream: TcpStream,
    protocol: u32,
    server_protocol: u32,
}

impl Client {
    pub fn connect(address: &str) -> Result<Self, SdkError> {
        let unreachable = |source| SdkError::Unreachable { address: address.to_string(), source };
        let addr: SocketAddr = address
            .to_socket_addrs()
            .map_err(unreachable)?
            .next()
            .ok_or_else(|| unreachable(std::io::Error::other("no address")))?;
        let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(unreachable)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.set_nodelay(true)?;
        let mut client = Client { stream, protocol: 0, server_protocol: 0 };

        client.send(0, packet::REQUEST_PROTOCOL_VERSION, &CLIENT_PROTOCOL.to_le_bytes())?;
        let reply = client.receive(packet::REQUEST_PROTOCOL_VERSION)?;
        client.server_protocol = wire::Reader::new(&reply).u32()?;
        client.protocol = client.server_protocol.min(CLIENT_PROTOCOL);
        client.send(0, packet::SET_CLIENT_NAME, &wire::c_string(CLIENT_NAME))?;
        Ok(client)
    }

    /// The protocol version both sides agreed on.
    pub fn protocol(&self) -> u32 {
        self.protocol
    }

    pub fn server_protocol(&self) -> u32 {
        self.server_protocol
    }

    pub fn controller_count(&mut self) -> Result<u32, SdkError> {
        self.send(0, packet::REQUEST_CONTROLLER_COUNT, &[])?;
        let reply = self.receive(packet::REQUEST_CONTROLLER_COUNT)?;
        wire::Reader::new(&reply).u32()
    }

    pub fn controller(&mut self, index: u32) -> Result<Device, SdkError> {
        self.send(index, packet::REQUEST_CONTROLLER_DATA, &self.protocol.to_le_bytes())?;
        let reply = self.receive(packet::REQUEST_CONTROLLER_DATA)?;
        Device::parse(index, &reply, self.protocol)
    }

    /// Switches device `device` to its mode number `index`, with `mode`'s
    /// settings. OpenRGB sends no reply; read the device back to check.
    pub fn update_mode(&mut self, device: u32, index: usize, mode: &Mode) -> Result<(), SdkError> {
        self.send(device, packet::UPDATE_MODE, &wire::update_mode(index, mode, self.protocol))
    }

    /// Sets every light on device `device`, one color per light, in order.
    /// OpenRGB sends no reply; read the device back to check.
    pub fn update_leds(&mut self, device: u32, colors: &[Rgb]) -> Result<(), SdkError> {
        self.send(device, packet::UPDATE_LEDS, &wire::update_leds(colors))
    }

    pub fn snapshot(&mut self) -> Result<Snapshot, SdkError> {
        let count = self.controller_count()?;
        let devices = (0..count).map(|i| self.controller(i)).collect::<Result<Vec<_>, _>>()?;
        Ok(Snapshot { server_protocol: self.server_protocol, devices })
    }

    fn send(&mut self, device: u32, id: u32, payload: &[u8]) -> Result<(), SdkError> {
        self.stream.write_all(&wire::encode(device, id, payload))?;
        Ok(())
    }

    /// Reads packets until one with `want` arrives. The server also sends
    /// unrequested notices, such as `DEVICE_LIST_UPDATED`, which are skipped.
    fn receive(&mut self, want: u32) -> Result<Vec<u8>, SdkError> {
        loop {
            let mut head = [0u8; HEADER_LEN];
            self.stream.read_exact(&mut head)?;
            let header = wire::decode_header(&head)?;
            if header.size > MAX_PAYLOAD {
                return Err(SdkError::Protocol(format!("a {} byte reply", header.size)));
            }
            let mut payload = vec![0u8; header.size as usize];
            self.stream.read_exact(&mut payload)?;
            if header.id == want {
                return Ok(payload);
            }
        }
    }
}

/// Connects, reads every device and disconnects.
pub fn snapshot(address: &str) -> Result<Snapshot, SdkError> {
    Client::connect(address)?.snapshot()
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;
    use crate::device::Kind;
    use crate::testing::{self, FakeServer};

    #[test]
    fn reads_every_device_and_negotiates_down() {
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71"), testing::ram("SMBus 0x73")]);
        let mut client = Client::connect(&server.address).unwrap();
        assert_eq!(client.server_protocol(), 6);
        assert_eq!(client.protocol(), CLIENT_PROTOCOL);
        let snap = client.snapshot().unwrap();
        assert_eq!(snap.server_protocol, 6);
        assert_eq!(snap.devices.len(), 2);
        assert_eq!(snap.devices[1].index, 1);
        assert_eq!(snap.devices[1].location, "SMBus 0x73");
        assert!(snap.devices.iter().all(|d| d.kind == Kind::Ram));

        // Reading asks only questions: no request that changes a light.
        let read_only = [
            packet::REQUEST_PROTOCOL_VERSION,
            packet::SET_CLIENT_NAME,
            packet::REQUEST_CONTROLLER_COUNT,
            packet::REQUEST_CONTROLLER_DATA,
        ];
        let seen = server.seen_ids();
        assert!(seen.iter().all(|id| read_only.contains(id)), "sent {seen:?}");
    }

    #[test]
    fn writes_reach_only_the_named_device() {
        let blue = Rgb { r: 0, g: 0, b: 255 };
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71"), testing::keyboard(testing::RED)]);
        let mut client = Client::connect(&server.address).unwrap();
        let direct = client.controller(1).unwrap().modes[0].clone();
        client.update_mode(1, 0, &direct).unwrap();
        client.update_leds(1, &[blue; 6]).unwrap();
        // A read after the writes also proves they were all received.
        assert_eq!(client.controller(1).unwrap().colors, vec![blue; 6]);
        assert_eq!(server.device(0), Device { index: 0, ..testing::ram("SMBus 0x71") });
        let writes: Vec<_> = server
            .state
            .lock()
            .unwrap()
            .seen
            .iter()
            .filter(|(id, _)| [packet::UPDATE_MODE, packet::UPDATE_LEDS].contains(id))
            .copied()
            .collect();
        assert_eq!(writes, [(packet::UPDATE_MODE, 1), (packet::UPDATE_LEDS, 1)]);
    }

    #[test]
    fn nothing_listening_is_unreachable() {
        // Bind and drop to get a port that is very likely closed.
        let address = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
        assert!(matches!(snapshot(&address), Err(SdkError::Unreachable { .. })));
    }
}
