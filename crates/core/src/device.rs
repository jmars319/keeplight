//! What OpenRGB reports about one lighting controller, decoded from its
//! `REQUEST_CONTROLLER_DATA` reply.

use serde::{Deserialize, Serialize};

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
    const ALL: [Kind; 19] = [
        Kind::Motherboard,
        Kind::Ram,
        Kind::GraphicsCard,
        Kind::Cooler,
        Kind::LedStrip,
        Kind::Keyboard,
        Kind::Mouse,
        Kind::Mousemat,
        Kind::Headset,
        Kind::HeadsetStand,
        Kind::Gamepad,
        Kind::Light,
        Kind::Speaker,
        Kind::Virtual,
        Kind::Storage,
        Kind::Case,
        Kind::Microphone,
        Kind::Accessory,
        Kind::Keypad,
    ];

    pub fn from_code(code: i32) -> Self {
        usize::try_from(code).ok().and_then(|i| Self::ALL.get(i).copied()).unwrap_or(Kind::Unknown)
    }

    /// OpenRGB's number for this kind; `Unknown` has none and maps to -1.
    pub fn code(self) -> i32 {
        Self::ALL.iter().position(|&k| k == self).map_or(-1, |i| i as i32)
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

/// A color as OpenRGB stores it: `0x00BBGGRR`. Saved as `"#rrggbb"`.
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

    pub fn to_wire(self) -> u32 {
        u32::from(self.r) | u32::from(self.g) << 8 | u32::from(self.b) << 16
    }

    pub fn is_black(self) -> bool {
        self == Rgb::default()
    }

    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.strip_prefix('#')?;
        if digits.len() != 6 || !digits.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
        Some(Self { r: byte(0)?, g: byte(2)?, b: byte(4)? })
    }
}

impl Serialize for Rgb {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Rgb::from_hex(&text).ok_or_else(|| serde::de::Error::custom(format!("not a #rrggbb color: {text}")))
    }
}

/// Mode flags from OpenRGB's `RGBController.h`.
pub mod flags {
    pub const MANUAL_SAVE: u32 = 1 << 8;
    pub const AUTOMATIC_SAVE: u32 = 1 << 9;
}

/// Where a mode takes its colors from, from OpenRGB's `RGBController.h`.
pub mod color_mode {
    /// The effect has fixed colors, such as a rainbow.
    pub const NONE: u32 = 0;
    /// Each light shows its own color from the device's color list.
    pub const PER_LED: u32 = 1;
    /// The effect uses the mode's own short color list.
    pub const MODE_SPECIFIC: u32 = 2;
    pub const RANDOM: u32 = 3;
}

/// One of a device's modes, with every field OpenRGB sends, so that it can be
/// sent back unchanged in `UPDATE_MODE`. Brightness fields exist only from
/// protocol 3; at lower versions they read as 0 and are not sent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Mode {
    pub name: String,
    pub value: i32,
    pub flags: u32,
    pub speed_min: u32,
    pub speed_max: u32,
    pub brightness_min: u32,
    pub brightness_max: u32,
    pub colors_min: u32,
    pub colors_max: u32,
    pub speed: u32,
    pub brightness: u32,
    pub direction: u32,
    pub color_mode: u32,
    pub colors: Vec<Rgb>,
}

/// What a person calls each device, in order: "RAM stick 1", "RAM stick 2",
/// "Keyboard". Devices of a kind are numbered only when there are several.
pub fn display_names(devices: &[Device]) -> Vec<String> {
    let total = |k: Kind| devices.iter().filter(|d| d.kind == k).count();
    let mut seen: Vec<(Kind, usize)> = Vec::new();
    devices
        .iter()
        .map(|d| {
            let n = match seen.iter_mut().find(|(k, _)| *k == d.kind) {
                Some((_, n)) => {
                    *n += 1;
                    *n
                }
                None => {
                    seen.push((d.kind, 1));
                    1
                }
            };
            if total(d.kind) > 1 { format!("{} {n}", d.kind.label()) } else { d.kind.label().to_string() }
        })
        .collect()
}

/// A device's id for the command line and AI tools: its display name in
/// lower case with dashes, such as "ram-stick-1".
pub fn device_id(display_name: &str) -> String {
    display_name.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-")
}

/// The color most of `colors` show, leaving black (off) out. Ties go to the
/// first color seen, so the answer is stable.
fn most_common(colors: &[Rgb]) -> Option<Rgb> {
    let mut counts: Vec<(Rgb, usize)> = Vec::new();
    for c in colors.iter().filter(|c| !c.is_black()) {
        match counts.iter_mut().find(|(k, _)| k == c) {
            Some((_, n)) => *n += 1,
            None => counts.push((*c, 1)),
        }
    }
    counts
        .into_iter()
        .fold(None, |best: Option<(Rgb, usize)>, (c, n)| match best {
            Some((_, bn)) if bn >= n => best,
            _ => Some((c, n)),
        })
        .map(|(c, _)| c)
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
    /// OpenRGB's index for this controller in the current device list. It can
    /// change when devices come and go, so saved setups match on name and
    /// location instead.
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
    pub fn active(&self) -> Option<&Mode> {
        self.active_mode.and_then(|i| self.modes.get(i))
    }

    pub fn active_mode_name(&self) -> Option<&str> {
        self.active().map(|m| m.name.as_str())
    }

    pub fn mode_index(&self, name: &str) -> Option<usize> {
        self.modes.iter().position(|m| m.name == name)
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

    /// The colors the device is showing: its own per-light list in a per-light
    /// mode, the mode's colors in a mode that has its own, and nothing for an
    /// effect with fixed colors.
    pub fn shown_colors(&self) -> &[Rgb] {
        match self.active().map(|m| m.color_mode) {
            Some(color_mode::MODE_SPECIFIC) => self.active().map_or(&[], |m| &m.colors),
            Some(color_mode::NONE | color_mode::RANDOM) => &[],
            _ => &self.colors,
        }
    }

    /// Which of the device's lights belong to zone `zone`: OpenRGB lists the
    /// lights zone by zone, in zone order. `None` when there is no such zone
    /// or the zones don't add up to the device's lights.
    pub fn zone_range(&self, zone: usize) -> Option<std::ops::Range<usize>> {
        let counts: Vec<usize> = self.zones.iter().map(|z| z.led_count as usize).collect();
        if counts.iter().sum::<usize>() != self.led_count() || zone >= counts.len() {
            return None;
        }
        let start: usize = counts[..zone].iter().sum();
        Some(start..start + counts[zone])
    }

    /// The zone named `wanted` (its name, or its id such as "front-logo"),
    /// ignoring case.
    pub fn zone_index(&self, wanted: &str) -> Option<usize> {
        let wanted = wanted.trim();
        self.zones.iter().position(|z| z.name.eq_ignore_ascii_case(wanted) || device_id(&z.name) == device_id(wanted))
    }

    /// The color most of zone `zone`'s lit lights show, like [`Device::lit_color`].
    pub fn zone_color(&self, zone: usize) -> Option<Rgb> {
        let range = self.zone_range(zone)?;
        if self.active().is_some_and(|m| m.color_mode != color_mode::PER_LED) {
            return self.lit_color();
        }
        most_common(&self.colors[range])
    }

    /// The name of the mode when the device is running a lighting mode of its
    /// own, such as "Onboard" or a rainbow, which colors the lights itself.
    /// OpenRGB's per-light colors are then stale or blank, so they say nothing
    /// about what the lights show.
    pub fn own_lighting(&self) -> Option<&str> {
        let m = self.active()?;
        let own = matches!(m.color_mode, color_mode::NONE | color_mode::RANDOM) && !m.name.eq_ignore_ascii_case("off");
        own.then_some(m.name.as_str())
    }

    /// The color most of its lit LEDs show, or `None` when every LED is off
    /// (black), it is in its "Off" mode, or it has none. A mode with colors of
    /// its own, such as Static, shows those rather than each light's own.
    /// Off LEDs are left out of the count, so a keyboard lit only on a few
    /// keys still reports those keys' color.
    pub fn lit_color(&self) -> Option<Rgb> {
        let active = self.active();
        // Off shows nothing; a mode of its own shows colors OpenRGB can't see.
        if active.is_some_and(|m| m.name.eq_ignore_ascii_case("off")) || self.own_lighting().is_some() {
            return None;
        }
        let shown = match active {
            Some(m) if m.color_mode == color_mode::MODE_SPECIFIC => &m.colors,
            _ => &self.colors,
        };
        most_common(shown)
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
            modes.push(parse_mode(&mut r, protocol)?);
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

pub(crate) fn parse_mode(r: &mut Reader, protocol: u32) -> Result<Mode, SdkError> {
    let mut m = Mode { name: r.string()?, value: r.i32()?, flags: r.u32()?, ..Mode::default() };
    m.speed_min = r.u32()?;
    m.speed_max = r.u32()?;
    if protocol >= 3 {
        m.brightness_min = r.u32()?;
        m.brightness_max = r.u32()?;
    }
    m.colors_min = r.u32()?;
    m.colors_max = r.u32()?;
    m.speed = r.u32()?;
    if protocol >= 3 {
        m.brightness = r.u32()?;
    }
    m.direction = r.u32()?;
    m.color_mode = r.u32()?;
    let count = r.u16()? as usize;
    m.colors = (0..count).map(|_| r.u32().map(Rgb::from_wire)).collect::<Result<_, _>>()?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, RED};

    fn round_trip(d: &Device) -> Device {
        Device::parse(d.index, &testing::encode_device(d, 3), 3).unwrap()
    }

    #[test]
    fn parses_a_ram_stick() {
        let d = round_trip(&testing::ram("SMBus 0x71"));
        assert_eq!(d.kind, Kind::Ram);
        assert_eq!(d.name, "ENE DRAM");
        assert_eq!(d.vendor, "ENE");
        assert_eq!(d.location, "SMBus 0x71");
        assert_eq!(d.active_mode_name(), Some("Chase Fade"));
        assert_eq!(d.led_count(), 8);
        assert_eq!(d.lit_color(), Some(RED));
        assert!(!d.can_store_color());
    }

    #[test]
    fn keeps_every_mode_field() {
        let mut d = testing::board();
        let m = &mut d.modes[1];
        (m.speed_min, m.speed_max, m.brightness_min, m.brightness_max) = (1, 2, 3, 4);
        (m.colors_min, m.colors_max, m.speed, m.brightness, m.direction) = (5, 6, 7, 8, 9);
        assert_eq!(round_trip(&d), d);
    }

    #[test]
    fn reads_a_keyboard_grid() {
        let d = round_trip(&testing::keyboard(Rgb::default()));
        assert_eq!(d.kind, Kind::Keyboard);
        assert_eq!(d.zones[0].matrix, Some((3, 2)));
        assert_eq!(d.lit_color(), None);
    }

    #[test]
    fn a_few_lit_keys_still_count_as_lit() {
        let green = Rgb { r: 0, g: 255, b: 0 };
        let mut d = testing::keyboard(Rgb::default());
        d.colors[2] = green;
        d.colors[3] = green;
        assert_eq!(round_trip(&d).lit_color(), Some(green));
    }

    #[test]
    fn a_saving_mode_means_the_device_keeps_its_color() {
        let mut d = testing::ram("USB");
        d.modes.push(Mode { name: "Static".into(), flags: flags::MANUAL_SAVE, ..Mode::default() });
        assert!(round_trip(&d).can_store_color());
    }

    #[test]
    fn the_swatch_color_follows_the_mode() {
        let mut board = testing::board();
        assert_eq!(board.lit_color(), Some(RED));
        board.modes[1].colors = vec![Rgb::default()];
        assert_eq!(board.lit_color(), None, "black Static is dark even though each light still reads red");
        let mut off = testing::ram("SMBus");
        off.modes.push(Mode { name: "Off".into(), ..Mode::default() });
        off.active_mode = Some(3);
        assert_eq!(off.lit_color(), None);
    }

    #[test]
    fn a_mode_of_its_own_hides_stale_colors() {
        let mut kb = testing::keyboard(RED);
        assert_eq!(kb.own_lighting(), None);
        kb.active_mode = Some(1);
        assert_eq!(kb.own_lighting(), Some("Onboard"));
        assert_eq!(kb.lit_color(), None, "the red OpenRGB still reports is stale");
    }

    #[test]
    fn shown_colors_follow_the_mode() {
        assert_eq!(testing::board().shown_colors(), &[RED]);
        assert_eq!(testing::ram("SMBus").shown_colors().len(), 8);
        let mut rainbow = testing::ram("SMBus");
        rainbow.active_mode = Some(1);
        assert!(rainbow.shown_colors().is_empty());
    }

    #[test]
    fn out_of_range_active_mode_is_none() {
        let bytes = testing::encode_with_active(&testing::ram("SMBus"), 3, 9);
        assert_eq!(Device::parse(0, &bytes, 3).unwrap().active_mode, None);
    }

    #[test]
    fn truncated_payload_is_an_error() {
        let bytes = testing::encode_device(&testing::ram("SMBus"), 3);
        assert!(Device::parse(0, &bytes[..bytes.len() - 3], 3).is_err());
    }

    #[test]
    fn devices_are_named_and_numbered_by_kind() {
        let devices = [testing::ram("a"), testing::ram("b"), testing::keyboard(RED)];
        assert_eq!(display_names(&devices), ["RAM stick 1", "RAM stick 2", "Keyboard"]);
        assert_eq!(device_id("RAM stick 2"), "ram-stick-2");
    }

    #[test]
    fn colors_are_bgr_on_the_wire() {
        let c = Rgb::from_wire(0x00336699);
        assert_eq!(c, Rgb { r: 0x99, g: 0x66, b: 0x33 });
        assert_eq!(c.to_wire(), 0x00336699);
    }

    #[test]
    fn colors_save_as_hex() {
        assert_eq!(Rgb { r: 255, g: 8, b: 0 }.hex(), "#ff0800");
        assert_eq!(Rgb::from_hex("#ff0800"), Some(Rgb { r: 255, g: 8, b: 0 }));
        assert_eq!(Rgb::from_hex("ff0800"), None);
        assert_eq!(Rgb::from_hex("#ff08"), None);
        assert_eq!(Rgb::from_hex("#gg0000"), None);
    }

    #[test]
    fn kinds_map_both_ways() {
        assert_eq!(Kind::from_code(2), Kind::GraphicsCard);
        assert_eq!(Kind::GraphicsCard.code(), 2);
        assert_eq!(Kind::from_code(-1), Kind::Unknown);
        assert_eq!(Kind::from_code(99), Kind::Unknown);
        assert_eq!(Kind::Unknown.code(), -1);
    }
}
