//! What OpenRGB reports about one lighting controller, decoded from its
//! `REQUEST_CONTROLLER_DATA` reply.

use crate::sdk::SdkError;
use crate::sdk::wire::Reader;

/// OpenRGB's device types, in its numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Motherboard,
    Ram,
    GraphicsCard,
    Cooler,
    LedStrip,
    Keyboard,
    Mouse,
    Mousemat,
    Headset,
    HeadsetStand,
    Gamepad,
    Light,
    Speaker,
    Virtual,
    Storage,
    Case,
    Microphone,
    Accessory,
    Keypad,
    Unknown,
}

impl Kind {
    pub fn from_code(code: i32) -> Self {
        use Kind::*;
        const ALL: [Kind; 19] = [
            Motherboard,
            Ram,
            GraphicsCard,
            Cooler,
            LedStrip,
            Keyboard,
            Mouse,
            Mousemat,
            Headset,
            HeadsetStand,
            Gamepad,
            Light,
            Speaker,
            Virtual,
            Storage,
            Case,
            Microphone,
            Accessory,
            Keypad,
        ];
        usize::try_from(code).ok().and_then(|i| ALL.get(i).copied()).unwrap_or(Unknown)
    }

    /// What a person calls it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Motherboard => "Motherboard",
            Kind::Ram => "RAM stick",
            Kind::GraphicsCard => "Graphics card",
            Kind::Cooler => "Cooler",
            Kind::LedStrip => "LED strip",
            Kind::Keyboard => "Keyboard",
            Kind::Mouse => "Mouse",
            Kind::Mousemat => "Mouse pad",
            Kind::Headset => "Headset",
            Kind::HeadsetStand => "Headset stand",
            Kind::Gamepad => "Controller",
            Kind::Light => "Light",
            Kind::Speaker => "Speaker",
            Kind::Virtual => "Virtual device",
            Kind::Storage => "Drive",
            Kind::Case => "Case",
            Kind::Microphone => "Microphone",
            Kind::Accessory => "Accessory",
            Kind::Keypad => "Keypad",
            Kind::Unknown => "Device",
        }
    }
}

/// A color as OpenRGB stores it: `0x00BBGGRR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub fn from_wire(v: u32) -> Self {
        Self { r: v as u8, g: (v >> 8) as u8, b: (v >> 16) as u8 }
    }

    pub fn is_black(self) -> bool {
        self == Rgb::default()
    }
}

/// Mode flags from OpenRGB's `RGBController.h`.
pub mod flags {
    pub const MANUAL_SAVE: u32 = 1 << 8;
    pub const AUTOMATIC_SAVE: u32 = 1 << 9;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    pub name: String,
    pub flags: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    pub name: String,
    pub led_count: u32,
    /// Width and height of the zone's LED grid, when it reports one. Only
    /// keyboards and similar devices do; everything else is a plain list.
    pub matrix: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// OpenRGB's index for this controller in the current device list.
    pub index: u32,
    pub kind: Kind,
    pub name: String,
    pub vendor: String,
    pub location: String,
    pub modes: Vec<Mode>,
    pub active_mode: Option<usize>,
    pub zones: Vec<Zone>,
    pub colors: Vec<Rgb>,
}

impl Device {
    pub fn active_mode_name(&self) -> Option<&str> {
        self.active_mode.and_then(|i| self.modes.get(i)).map(|m| m.name.as_str())
    }

    pub fn led_count(&self) -> usize {
        self.colors.len()
    }

    /// Whether any of the device's modes can be stored in the device itself.
    /// OpenRGB shows "Saving Not Supported" when none can; such a device loses
    /// its color whenever it loses power and has to be set again.
    pub fn can_store_color(&self) -> bool {
        self.modes.iter().any(|m| m.flags & (flags::MANUAL_SAVE | flags::AUTOMATIC_SAVE) != 0)
    }

    /// The color most of its lit LEDs show, or `None` when every LED is off
    /// (black) or it has none. Off LEDs are left out of the count, so a
    /// keyboard lit only on a few keys still reports those keys' color.
    pub fn lit_color(&self) -> Option<Rgb> {
        let mut counts: Vec<(Rgb, usize)> = Vec::new();
        for c in self.colors.iter().filter(|c| !c.is_black()) {
            match counts.iter_mut().find(|(k, _)| k == c) {
                Some((_, n)) => *n += 1,
                None => counts.push((*c, 1)),
            }
        }
        // Ties go to the first color seen, so the answer is stable.
        counts
            .into_iter()
            .fold(None, |best: Option<(Rgb, usize)>, (c, n)| match best {
                Some((_, bn)) if bn >= n => best,
                _ => Some((c, n)),
            })
            .map(|(c, _)| c)
    }

    /// Decodes a `REQUEST_CONTROLLER_DATA` payload sent at `protocol`
    /// (0 to 3; Keeplight never negotiates higher).
    pub fn parse(index: u32, payload: &[u8], protocol: u32) -> Result<Self, SdkError> {
        let mut r = Reader::new(payload);
        let _data_size = r.u32()?;
        let kind = Kind::from_code(r.i32()?);
        let name = r.string()?;
        let vendor = if protocol >= 1 { r.string()? } else { String::new() };
        let _description = r.string()?;
        let _version = r.string()?;
        let _serial = r.string()?;
        let location = r.string()?;

        let mode_count = r.u16()? as usize;
        let active = r.i32()?;
        let mut modes = Vec::with_capacity(mode_count);
        for _ in 0..mode_count {
            let name = r.string()?;
            let _value = r.i32()?;
            let flags = r.u32()?;
            // speed min/max, [brightness min/max], colors min/max, speed,
            // [brightness], direction, color mode.
            let words = if protocol >= 3 { 10 } else { 7 };
            r.skip(4 * words)?;
            let colors = r.u16()? as usize;
            r.skip(4 * colors)?;
            modes.push(Mode { name, flags });
        }

        let zone_count = r.u16()? as usize;
        let mut zones = Vec::with_capacity(zone_count);
        for _ in 0..zone_count {
            let name = r.string()?;
            let _zone_type = r.i32()?;
            let _leds_min = r.u32()?;
            let _leds_max = r.u32()?;
            let led_count = r.u32()?;
            let matrix_len = r.u16()? as usize;
            let matrix = if matrix_len >= 8 {
                let height = r.u32()?;
                let width = r.u32()?;
                r.skip(matrix_len - 8)?;
                Some((width, height))
            } else {
                r.skip(matrix_len)?;
                None
            };
            zones.push(Zone { name, led_count, matrix });
        }

        let led_count = r.u16()? as usize;
        for _ in 0..led_count {
            let _led_name = r.string()?;
            let _value = r.u32()?;
        }
        let color_count = r.u16()? as usize;
        let mut colors = Vec::with_capacity(color_count);
        for _ in 0..color_count {
            colors.push(Rgb::from_wire(r.u32()?));
        }

        let active_mode = usize::try_from(active).ok().filter(|&i| i < modes.len());
        Ok(Device { index, kind, name, vendor, location, modes, active_mode, zones, colors })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A zone as (name, light count, optional width and height).
    pub type FixtureZone = (&'static str, u32, Option<(u32, u32)>);

    /// Builds a controller payload the way OpenRGB serializes one at
    /// protocol 3, so tests can feed the parser and the fake server.
    pub struct Fixture {
        pub kind: i32,
        pub name: &'static str,
        pub vendor: &'static str,
        pub location: &'static str,
        pub modes: Vec<(&'static str, u32)>,
        pub active: i32,
        pub zones: Vec<FixtureZone>,
        pub colors: Vec<u32>,
    }

    fn put_str(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&((s.len() + 1) as u16).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }

    impl Fixture {
        pub fn ram(name: &'static str, location: &'static str, color: u32) -> Self {
            Fixture {
                kind: 1,
                name,
                vendor: "ENE",
                location,
                modes: vec![("Direct", 0x20), ("Static", 0x40), ("Chase Fade", 0x40)],
                active: 2,
                zones: vec![("DRAM", 8, None)],
                colors: vec![color; 8],
            }
        }

        pub fn encode(&self) -> Vec<u8> {
            let mut b = Vec::new();
            b.extend_from_slice(&0u32.to_le_bytes());
            b.extend_from_slice(&self.kind.to_le_bytes());
            put_str(&mut b, self.name);
            put_str(&mut b, self.vendor);
            put_str(&mut b, "description");
            put_str(&mut b, "1.0");
            put_str(&mut b, "");
            put_str(&mut b, self.location);
            b.extend_from_slice(&(self.modes.len() as u16).to_le_bytes());
            b.extend_from_slice(&self.active.to_le_bytes());
            for (name, flags) in &self.modes {
                put_str(&mut b, name);
                b.extend_from_slice(&0i32.to_le_bytes());
                b.extend_from_slice(&flags.to_le_bytes());
                for _ in 0..10 {
                    b.extend_from_slice(&0u32.to_le_bytes());
                }
                b.extend_from_slice(&1u16.to_le_bytes());
                b.extend_from_slice(&0x0000ffu32.to_le_bytes());
            }
            b.extend_from_slice(&(self.zones.len() as u16).to_le_bytes());
            for (name, leds, matrix) in &self.zones {
                put_str(&mut b, name);
                b.extend_from_slice(&0i32.to_le_bytes());
                b.extend_from_slice(&leds.to_le_bytes());
                b.extend_from_slice(&leds.to_le_bytes());
                b.extend_from_slice(&leds.to_le_bytes());
                match matrix {
                    Some((w, h)) => {
                        let cells = (w * h) as usize;
                        b.extend_from_slice(&((8 + 4 * cells) as u16).to_le_bytes());
                        b.extend_from_slice(&h.to_le_bytes());
                        b.extend_from_slice(&w.to_le_bytes());
                        for i in 0..cells {
                            b.extend_from_slice(&(i as u32).to_le_bytes());
                        }
                    }
                    None => b.extend_from_slice(&0u16.to_le_bytes()),
                }
            }
            b.extend_from_slice(&(self.colors.len() as u16).to_le_bytes());
            for i in 0..self.colors.len() {
                put_str(&mut b, &format!("LED {i}"));
                b.extend_from_slice(&0u32.to_le_bytes());
            }
            b.extend_from_slice(&(self.colors.len() as u16).to_le_bytes());
            for c in &self.colors {
                b.extend_from_slice(&c.to_le_bytes());
            }
            let size = b.len() as u32;
            b[..4].copy_from_slice(&size.to_le_bytes());
            b
        }
    }

    #[test]
    fn parses_a_ram_stick() {
        let bytes = Fixture::ram("ENE DRAM", "I2C: SMBus 0x71", 0x0000ff).encode();
        let d = Device::parse(0, &bytes, 3).unwrap();
        assert_eq!(d.kind, Kind::Ram);
        assert_eq!(d.name, "ENE DRAM");
        assert_eq!(d.vendor, "ENE");
        assert_eq!(d.location, "I2C: SMBus 0x71");
        assert_eq!(d.active_mode_name(), Some("Chase Fade"));
        assert_eq!(d.led_count(), 8);
        assert_eq!(d.lit_color(), Some(Rgb { r: 255, g: 0, b: 0 }));
        assert!(!d.can_store_color());
    }

    #[test]
    fn reads_a_keyboard_grid() {
        let f = Fixture {
            kind: 5,
            name: "Keyboard",
            vendor: "SteelSeries",
            location: "HID",
            modes: vec![("Direct", 0x20), ("Onboard", 0x10)],
            active: 0,
            zones: vec![("Keyboard", 6, Some((3, 2)))],
            colors: vec![0; 6],
        };
        let d = Device::parse(4, &f.encode(), 3).unwrap();
        assert_eq!(d.kind, Kind::Keyboard);
        assert_eq!(d.zones[0].matrix, Some((3, 2)));
        assert_eq!(d.lit_color(), None);
    }

    #[test]
    fn a_few_lit_keys_still_count_as_lit() {
        let green = 0x00ff00;
        let mut colors = vec![0; 10];
        colors[2] = green;
        colors[3] = green;
        let mut f = Fixture::ram("Keyboard", "HID", 0);
        f.colors = colors;
        let d = Device::parse(0, &f.encode(), 3).unwrap();
        assert_eq!(d.lit_color(), Some(Rgb { r: 0, g: 255, b: 0 }));
    }

    #[test]
    fn a_saving_mode_means_the_device_keeps_its_color() {
        let mut f = Fixture::ram("Board", "USB", 0);
        f.kind = 0;
        f.modes.push(("Static", 0x140));
        assert!(Device::parse(0, &f.encode(), 3).unwrap().can_store_color());
    }

    #[test]
    fn out_of_range_active_mode_is_none() {
        let mut f = Fixture::ram("RAM", "SMBus", 0);
        f.active = 9;
        assert_eq!(Device::parse(0, &f.encode(), 3).unwrap().active_mode, None);
    }

    #[test]
    fn truncated_payload_is_an_error() {
        let bytes = Fixture::ram("RAM", "SMBus", 0).encode();
        assert!(Device::parse(0, &bytes[..bytes.len() - 3], 3).is_err());
    }

    #[test]
    fn colors_are_bgr_on_the_wire() {
        assert_eq!(Rgb::from_wire(0x00336699), Rgb { r: 0x99, g: 0x66, b: 0x33 });
    }

    #[test]
    fn unknown_kinds_map_to_unknown() {
        assert_eq!(Kind::from_code(2), Kind::GraphicsCard);
        assert_eq!(Kind::from_code(-1), Kind::Unknown);
        assert_eq!(Kind::from_code(99), Kind::Unknown);
    }
}
