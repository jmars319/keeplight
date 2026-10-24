//! A synchronous client for OpenRGB's SDK server.
//!
//! Written from OpenRGB's protocol description rather than taken from an
//! existing crate, because the maintained one is GPL-2.0-only and this app is
//! GPL-3.0-only (`docs/adr/0001-build-on-openrgb.md`).

pub mod wire;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::device::Device;
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
    use std::thread;

    use super::*;
    use crate::device::Kind;
    use crate::device::tests::Fixture;

    /// A stand-in OpenRGB server that answers the read-only requests from
    /// fixtures, and records every packet id it was sent.
    fn fake_server(server_protocol: u32, devices: Vec<Vec<u8>>) -> (String, thread::JoinHandle<Vec<u32>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let handle = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut seen = Vec::new();
            loop {
                let mut head = [0u8; HEADER_LEN];
                if s.read_exact(&mut head).is_err() {
                    return seen;
                }
                let h = wire::decode_header(&head).unwrap();
                let mut payload = vec![0u8; h.size as usize];
                s.read_exact(&mut payload).unwrap();
                seen.push(h.id);
                let reply = match h.id {
                    packet::REQUEST_PROTOCOL_VERSION => Some(server_protocol.to_le_bytes().to_vec()),
                    packet::REQUEST_CONTROLLER_COUNT => {
                        // An unrequested notice first, which the client must skip.
                        s.write_all(&wire::encode(0, packet::DEVICE_LIST_UPDATED, &[])).unwrap();
                        Some((devices.len() as u32).to_le_bytes().to_vec())
                    }
                    packet::REQUEST_CONTROLLER_DATA => Some(devices[h.device as usize].clone()),
                    _ => None,
                };
                if let Some(r) = reply {
                    s.write_all(&wire::encode(h.device, h.id, &r)).unwrap();
                }
            }
        });
        (address, handle)
    }

    #[test]
    fn reads_every_device_and_negotiates_down() {
        let devices = vec![
            Fixture::ram("ENE DRAM", "SMBus 0x71", 0x0000ff).encode(),
            Fixture::ram("ENE DRAM", "SMBus 0x73", 0x0000ff).encode(),
        ];
        let (address, server) = fake_server(6, devices);
        let mut client = Client::connect(&address).unwrap();
        assert_eq!(client.server_protocol(), 6);
        assert_eq!(client.protocol(), CLIENT_PROTOCOL);
        let snap = client.snapshot().unwrap();
        drop(client);
        assert_eq!(snap.server_protocol, 6);
        assert_eq!(snap.devices.len(), 2);
        assert_eq!(snap.devices[1].location, "SMBus 0x73");
        assert!(snap.devices.iter().all(|d| d.kind == Kind::Ram));

        // Only questions were asked: no request that changes a light.
        let seen = server.join().unwrap();
        let read_only = [
            packet::REQUEST_PROTOCOL_VERSION,
            packet::SET_CLIENT_NAME,
            packet::REQUEST_CONTROLLER_COUNT,
            packet::REQUEST_CONTROLLER_DATA,
        ];
        assert!(seen.iter().all(|id| read_only.contains(id)), "sent {seen:?}");
    }

    #[test]
    fn nothing_listening_is_unreachable() {
        // Bind and drop to get a port that is very likely closed.
        let address = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
        assert!(matches!(snapshot(&address), Err(SdkError::Unreachable { .. })));
    }
}
