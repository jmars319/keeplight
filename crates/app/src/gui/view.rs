//! Turns what OpenRGB reported into the words the window shows. Plain Rust,
//! so it is tested without a window.

use std::collections::HashMap;

use keeplight_core::device::{Device, Kind};
use keeplight_core::sdk::Snapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    /// The device list.
    Devices,
    /// A full-window message instead of the list.
    Message,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub screen: Screen,
    pub chip: String,
    pub chip_warn: bool,
    pub heading: String,
    pub message: String,
    pub summary: String,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// What a person calls it, such as "RAM stick 2".
    pub name: String,
    /// The product name OpenRGB reports.
    pub product: String,
    /// Light count and current effect.
    pub meta: String,
    /// Zone names with their light counts, when there is more than one zone.
    pub zones: String,
    pub note: String,
    /// The color most of its lit lights show; `None` only when every light
    /// is off.
    pub swatch: Option<(u8, u8, u8)>,
}

pub fn build(result: &Result<Snapshot, String>, address: &str) -> View {
    let snapshot = match result {
        Ok(s) => s,
        Err(reason) => {
            return message(
                "Not connected",
                "Can't reach OpenRGB",
                &format!(
                    "Keeplight reads your lights through OpenRGB, which should be running on this PC and \
                     listening at {address}. Start the OpenRGB service, or turn on its SDK server. This window \
                     checks again every few seconds.\n\n{reason}"
                ),
            );
        }
    };
    if snapshot.devices.is_empty() {
        return message(
            "Connected",
            "OpenRGB hasn't found any lights yet",
            "OpenRGB is running but reports no devices. It may still be scanning after startup. This window \
             checks again every few seconds.",
        );
    }
    let lights: usize = snapshot.devices.iter().map(Device::led_count).sum();
    View {
        screen: Screen::Devices,
        chip: "OpenRGB connected".into(),
        chip_warn: false,
        heading: String::new(),
        message: String::new(),
        summary: format!("{} · {}", plural(snapshot.devices.len(), "device"), plural(lights, "light")),
        rows: rows(&snapshot.devices),
    }
}

fn rows(devices: &[Device]) -> Vec<Row> {
    let mut totals: HashMap<Kind, usize> = HashMap::new();
    for d in devices {
        *totals.entry(d.kind).or_default() += 1;
    }
    let mut seen: HashMap<Kind, usize> = HashMap::new();
    devices
        .iter()
        .map(|d| {
            let n = seen.entry(d.kind).or_default();
            *n += 1;
            let name =
                if totals[&d.kind] > 1 { format!("{} {}", d.kind.label(), n) } else { d.kind.label().to_string() };
            let mut meta = plural(d.led_count(), "light");
            if let Some(mode) = d.active_mode_name() {
                meta.push_str(" · ");
                meta.push_str(mode);
            }
            let zones = if d.zones.len() > 1 {
                d.zones.iter().map(|z| format!("{} {}", z.name, z.led_count)).collect::<Vec<_>>().join(" · ")
            } else {
                String::new()
            };
            let note = if d.can_store_color() {
                String::new()
            } else {
                "Can't save a color to itself, so it resets whenever it loses power.".to_string()
            };
            Row { name, product: d.name.clone(), meta, zones, note, swatch: d.lit_color().map(|c| (c.r, c.g, c.b)) }
        })
        .collect()
}

fn message(chip: &str, heading: &str, message: &str) -> View {
    View {
        screen: Screen::Message,
        chip: chip.to_string(),
        chip_warn: true,
        heading: heading.to_string(),
        message: message.to_string(),
        summary: String::new(),
        rows: Vec::new(),
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

#[cfg(test)]
mod tests {
    use keeplight_core::device::{Mode, Rgb, Zone};

    use super::*;

    fn device(kind: Kind, name: &str, mode: &str, colors: Vec<Rgb>, saves: bool) -> Device {
        Device {
            index: 0,
            kind,
            name: name.into(),
            vendor: String::new(),
            location: String::new(),
            modes: vec![Mode { name: mode.into(), flags: if saves { 1 << 8 } else { 0 } }],
            active_mode: Some(0),
            zones: vec![Zone { name: "Main".into(), led_count: colors.len() as u32, matrix: None }],
            colors,
        }
    }

    const RED: Rgb = Rgb { r: 255, g: 0, b: 0 };

    #[test]
    fn unreachable_server_explains_where_it_looked() {
        let v = build(&Err("refused".into()), "127.0.0.1:6742");
        assert_eq!(v.screen, Screen::Message);
        assert!(v.chip_warn);
        assert!(v.message.contains("127.0.0.1:6742"));
        assert!(v.message.contains("refused"));
    }

    #[test]
    fn no_devices_is_its_own_message() {
        let v = build(&Ok(Snapshot { server_protocol: 6, devices: vec![] }), "a");
        assert_eq!(v.screen, Screen::Message);
        assert!(v.heading.contains("hasn't found any lights"));
    }

    #[test]
    fn devices_get_plain_names_numbered_by_kind() {
        let snap = Snapshot {
            server_protocol: 6,
            devices: vec![
                device(Kind::Ram, "ENE DRAM", "Chase Fade", vec![RED; 8], false),
                device(Kind::Ram, "ENE DRAM", "Chase Fade", vec![RED; 8], false),
                device(Kind::GraphicsCard, "EVGA RTX", "Direct", vec![RED; 4], true),
                device(Kind::Keyboard, "Apex", "Direct", vec![Rgb::default(); 2], false),
            ],
        };
        let v = build(&Ok(snap), "a");
        assert_eq!(v.screen, Screen::Devices);
        assert_eq!(v.summary, "4 devices · 22 lights");
        let names: Vec<_> = v.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["RAM stick 1", "RAM stick 2", "Graphics card", "Keyboard"]);
        assert_eq!(v.rows[0].meta, "8 lights · Chase Fade");
        assert_eq!(v.rows[0].swatch, Some((255, 0, 0)));
        assert!(v.rows[0].note.contains("resets"));
        assert!(v.rows[2].note.is_empty());
        assert_eq!(v.rows[3].swatch, None);
    }

    #[test]
    fn mostly_dark_device_with_some_lit_keys_is_not_off() {
        let mut colors = vec![Rgb::default(); 100];
        colors[0] = RED;
        let snap =
            Snapshot { server_protocol: 6, devices: vec![device(Kind::Keyboard, "Apex", "Direct", colors, false)] };
        let v = build(&Ok(snap), "a");
        assert_eq!(v.rows[0].swatch, Some((255, 0, 0)));
    }

    #[test]
    fn several_zones_are_listed_with_their_counts() {
        let mut d = device(Kind::Motherboard, "Board", "Direct", vec![RED; 6], true);
        d.zones = vec![
            Zone { name: "Board".into(), led_count: 4, matrix: None },
            Zone { name: "Header 1".into(), led_count: 2, matrix: None },
        ];
        let v = build(&Ok(Snapshot { server_protocol: 6, devices: vec![d] }), "a");
        assert_eq!(v.rows[0].zones, "Board 4 · Header 1 2");
        assert_eq!(v.summary, "1 device · 6 lights");
    }
}
