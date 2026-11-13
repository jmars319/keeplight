//! A fake OpenRGB SDK server and sample devices, for tests and for the smoke
//! tests' `fake-openrgb` example. Built only for tests or with the `testing`
//! feature; never part of the app.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::device::{Device, Kind, Mode, Rgb, Zone, color_mode, parse_mode};
use crate::sdk::wire::{self, HEADER_LEN, Reader, Writer, packet};

pub const RED: Rgb = Rgb { r: 255, g: 0, b: 0 };

fn mode(name: &str, value: i32, color_mode: u32, colors: Vec<Rgb>) -> Mode {
    Mode { name: name.into(), value, color_mode, colors, ..Mode::default() }
}

/// A Mushkin RAM stick as OpenRGB reported it on 2026-11-13: no mode can be
/// saved to the stick, and it shows red Chase Fade, which colors each light
/// itself.
pub fn ram(location: &str) -> Device {
    Device {
        index: 0,
        kind: Kind::Ram,
        name: "ENE DRAM".into(),
        vendor: "ENE".into(),
        location: location.into(),
        modes: vec![
            mode("Direct", 0, color_mode::PER_LED, vec![]),
            mode("Rainbow", 5, color_mode::NONE, vec![]),
            mode("Chase Fade", 7, color_mode::PER_LED, vec![]),
        ],
        active_mode: Some(2),
        zones: vec![Zone { name: "DRAM".into(), led_count: 8, matrix: None }],
        colors: vec![RED; 8],
    }
}

/// A motherboard in red Static, a mode that takes its color from the mode's
/// own list rather than from each light.
pub fn board() -> Device {
    Device {
        index: 0,
        kind: Kind::Motherboard,
        name: "ASUS board".into(),
        vendor: "ASUS".into(),
        location: "HID: board".into(),
        modes: vec![
            mode("Direct", 0, color_mode::PER_LED, vec![]),
            mode("Static", 1, color_mode::MODE_SPECIFIC, vec![RED]),
        ],
        active_mode: Some(1),
        zones: vec![Zone { name: "Board".into(), led_count: 4, matrix: None }],
        colors: vec![RED; 4],
    }
}

/// A small keyboard in Direct mode, 3 by 2 keys, every key `color`.
pub fn keyboard(color: Rgb) -> Device {
    Device {
        index: 0,
        kind: Kind::Keyboard,
        name: "Apex".into(),
        vendor: "SteelSeries".into(),
        location: "HID: keyboard".into(),
        modes: vec![mode("Direct", 0, color_mode::PER_LED, vec![]), mode("Onboard", 1, color_mode::NONE, vec![])],
        active_mode: Some(0),
        zones: vec![Zone { name: "Keyboard".into(), led_count: 6, matrix: Some((3, 2)) }],
        colors: vec![color; 6],
    }
}

/// Serializes a device the way OpenRGB answers `REQUEST_CONTROLLER_DATA`.
pub fn encode_device(d: &Device, protocol: u32) -> Vec<u8> {
    encode_with_active(d, protocol, d.active_mode.map_or(-1, |i| i as i32))
}

/// [`encode_device`] with any active mode index, including invalid ones.
pub fn encode_with_active(d: &Device, protocol: u32, active: i32) -> Vec<u8> {
    let mut w = Writer::default();
    w.i32(d.kind.code()).string(&d.name);
    if protocol >= 1 {
        w.string(&d.vendor);
    }
    w.string("description").string("1.0").string("").string(&d.location);
    w.u16(d.modes.len() as u16).i32(active);
    for m in &d.modes {
        w.mode(m, protocol);
    }
    w.u16(d.zones.len() as u16);
    for z in &d.zones {
        w.string(&z.name).i32(0).u32(z.led_count).u32(z.led_count).u32(z.led_count);
        match z.matrix {
            Some((width, height)) => {
                let cells = width * height;
                w.u16((8 + 4 * cells) as u16).u32(height).u32(width);
                for i in 0..cells {
                    w.u32(i);
                }
            }
            None => {
                w.u16(0);
            }
        }
    }
    w.u16(d.colors.len() as u16);
    for i in 0..d.colors.len() {
        w.string(&format!("LED {i}")).u32(0);
    }
    w.colors(&d.colors);
    w.into_sized()
}

#[derive(Debug, Default)]
pub struct State {
    pub devices: Vec<Device>,
    /// Every request received, as (packet id, device index).
    pub seen: Vec<(u32, u32)>,
    /// Device indexes that accept writes but don't change, like a device that
    /// failed to take a color.
    pub stuck: Vec<u32>,
}

/// An OpenRGB stand-in on a random 127.0.0.1 port. It answers the read
/// requests from its devices and applies `UPDATE_MODE` and `UPDATE_LEDS` to
/// them, so a read after a write shows the change. Each connection gets its
/// own thread; the server lives until the process ends.
pub struct FakeServer {
    pub address: String,
    pub state: Arc<Mutex<State>>,
}

impl FakeServer {
    pub fn start(devices: Vec<Device>) -> Self {
        Self::start_on("127.0.0.1:0", devices)
    }

    pub fn start_on(bind: &str, devices: Vec<Device>) -> Self {
        let listener = TcpListener::bind(bind).expect("bind the fake OpenRGB server");
        let address = listener.local_addr().unwrap().to_string();
        let devices = devices.into_iter().enumerate().map(|(i, d)| Device { index: i as u32, ..d }).collect();
        let state = Arc::new(Mutex::new(State { devices, ..State::default() }));
        let shared = state.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = shared.clone();
                thread::spawn(move || serve(stream, &state));
            }
        });
        FakeServer { address, state }
    }

    pub fn seen_ids(&self) -> Vec<u32> {
        self.state.lock().unwrap().seen.iter().map(|&(id, _)| id).collect()
    }

    pub fn device(&self, index: usize) -> Device {
        self.state.lock().unwrap().devices[index].clone()
    }
}

fn serve(mut s: TcpStream, state: &Mutex<State>) {
    let mut protocol = 0;
    loop {
        let mut head = [0u8; HEADER_LEN];
        if s.read_exact(&mut head).is_err() {
            return;
        }
        let h = wire::decode_header(&head).unwrap();
        let mut payload = vec![0u8; h.size as usize];
        if s.read_exact(&mut payload).is_err() {
            return;
        }
        let mut st = state.lock().unwrap();
        st.seen.push((h.id, h.device));
        let reply = match h.id {
            packet::REQUEST_PROTOCOL_VERSION => {
                protocol = Reader::new(&payload).u32().unwrap_or(0).min(3);
                Some(6u32.to_le_bytes().to_vec())
            }
            packet::REQUEST_CONTROLLER_COUNT => {
                // An unrequested notice first, which clients must skip.
                let _ = s.write_all(&wire::encode(0, packet::DEVICE_LIST_UPDATED, &[]));
                Some((st.devices.len() as u32).to_le_bytes().to_vec())
            }
            packet::REQUEST_CONTROLLER_DATA => st.devices.get(h.device as usize).map(|d| encode_device(d, protocol)),
            packet::UPDATE_MODE | packet::UPDATE_LEDS if !st.stuck.contains(&h.device) => {
                if let Some(d) = st.devices.get_mut(h.device as usize) {
                    apply(d, h.id, &payload, protocol);
                }
                None
            }
            _ => None,
        };
        drop(st);
        if let Some(r) = reply
            && s.write_all(&wire::encode(h.device, h.id, &r)).is_err()
        {
            return;
        }
    }
}

fn apply(d: &mut Device, id: u32, payload: &[u8], protocol: u32) {
    let mut r = Reader::new(payload);
    let _size = r.u32();
    if id == packet::UPDATE_LEDS {
        let Ok(count) = r.u16() else { return };
        let colors: Result<Vec<_>, _> = (0..count).map(|_| r.u32().map(Rgb::from_wire)).collect();
        if let Ok(colors) = colors
            && colors.len() == d.colors.len()
        {
            d.colors = colors;
        }
        return;
    }
    // UPDATE_MODE: the index, then a mode in the controller-data layout.
    // OpenRGB keys on the index, and so does the fake.
    let Ok(index) = r.i32() else { return };
    let Ok(i) = usize::try_from(index) else { return };
    if let Ok(m) = parse_mode(&mut r, protocol)
        && i < d.modes.len()
    {
        d.modes[i] = m;
        d.active_mode = Some(i);
    }
}
