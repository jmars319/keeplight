//! Turns what OpenRGB reported, and what you chose to keep, into the words
//! the window shows. Plain Rust, so it is tested without a window.

use std::collections::HashMap;

use keeplight_core::device::{Device, Kind};
use keeplight_core::keep::{Fit, KeepError, Kept, Setup, is_dark};
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
    /// A line under the heading: the result of the last action, or a problem
    /// with the saved setup.
    pub notice: String,
    pub notice_warn: bool,
    pub rows: Vec<Row>,
    /// Whether the keeper runs at sign-in, with a sentence about it.
    pub sign_in: bool,
    pub sign_in_note: String,
    /// The keeper's last log summary, if it has run.
    pub last_run: String,
    /// Whether the keeper shows its icon in the notification area.
    pub tray_icon: bool,
}

/// The sign-in switch's state and sentence. `current` is the registered
/// startup command, `this_exe` the one this copy of Keeplight would register.
pub fn sign_in(current: Option<&str>, this_exe: Option<&str>, kept: usize) -> (bool, String) {
    let Some(current) = current else {
        let note = if kept > 0 {
            "Off: kept devices are put back only when you press Put back."
        } else {
            "Off. Keep a device first, then turn this on."
        };
        return (false, note.to_string());
    };
    if this_exe.is_some_and(|c| !c.eq_ignore_ascii_case(current)) {
        return (
            true,
            format!(
                "On, but sign-in starts another copy of Keeplight ({current}). Turn this off and on again to \
                 use this copy."
            ),
        );
    }
    let note = if kept > 0 {
        "Keeplight puts your kept devices back at sign-in, after sleep and when OpenRGB's devices change, without \
         opening this window."
    } else {
        "On, but nothing is kept yet, so there is nothing to put back."
    };
    (true, note.to_string())
}

/// Where a device stands against what you kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NotKept,
    Kept,
    Differs,
    /// Kept, but the device changed so the kept setup no longer fits.
    Changed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::NotKept => "",
            Status::Kept => "kept",
            Status::Differs => "differs",
            Status::Changed => "changed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Identifies the device to the actions: product name and location.
    pub name_key: String,
    pub location_key: String,
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
    pub status: Status,
    /// A sentence about the status, or why it can't be kept.
    pub status_text: String,
    pub can_keep: bool,
    /// Every light is off, so keeping it asks first.
    pub dark: bool,
    /// It was kept before, so "Keep it as before" can bring that back.
    pub has_before: bool,
    /// It runs a lighting mode of its own, so its colors are unknown.
    pub own_lighting: bool,
    /// What probably lights it, after "Lit by: ".
    pub lit_by: String,
    /// Another running app may fight Keeplight over it.
    pub lit_by_warn: bool,
}

/// The saved setup as the window sees it: read, or the reason it couldn't be.
pub type SetupView = Result<Setup, String>;

/// `running` holds the names of running programs, for guessing what lights
/// each device (`keeplight_core::lit_by`).
pub fn build(
    result: &Result<Snapshot, String>,
    setup: &SetupView,
    address: &str,
    notice: Option<&Notice>,
    running: &[String],
) -> View {
    let (notice, notice_warn) = match (setup, notice) {
        (Err(reason), _) => (format!("Your saved setup can't be used, so Keep and Put back are off. {reason}"), true),
        (Ok(_), Some(n)) => (n.text.clone(), n.warn),
        (Ok(_), None) => (String::new(), false),
    };
    let snapshot = match result {
        Ok(s) => s,
        Err(reason) => {
            let mut v = message(
                "Not connected",
                "Can't reach OpenRGB",
                &format!(
                    "Keeplight reads your lights through OpenRGB, which should be running on this PC and \
                     listening at {address}. Start the OpenRGB service, or turn on its SDK server. This window \
                     checks again every few seconds.\n\n{reason}"
                ),
            );
            (v.notice, v.notice_warn) = (notice, notice_warn);
            return v;
        }
    };
    if snapshot.devices.is_empty() {
        let mut v = message(
            "Connected",
            "OpenRGB hasn't found any lights yet",
            "OpenRGB is running but reports no devices. It may still be scanning after startup. This window \
             checks again every few seconds.",
        );
        (v.notice, v.notice_warn) = (notice, notice_warn);
        return v;
    }
    let lights: usize = snapshot.devices.iter().map(Device::led_count).sum();
    let kept = setup.as_ref().ok();
    let kept_count = kept.map_or(0, |s| snapshot.devices.iter().filter(|d| s.find(d).is_some()).count());
    let mut summary = format!("{} · {}", plural(snapshot.devices.len(), "device"), plural(lights, "light"));
    if kept_count > 0 {
        summary.push_str(&format!(" · {kept_count} kept"));
    }
    View {
        screen: Screen::Devices,
        chip: "OpenRGB connected".into(),
        chip_warn: false,
        heading: String::new(),
        message: String::new(),
        summary,
        notice,
        notice_warn,
        rows: rows(&snapshot.devices, kept, running),
        sign_in: false,
        sign_in_note: String::new(),
        last_run: String::new(),
        tray_icon: false,
    }
}

/// The result of the last Keep or Put back, shown until the next action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub warn: bool,
}

fn rows(devices: &[Device], setup: Option<&Setup>, running: &[String]) -> Vec<Row> {
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
            let (status, status_text) = status(d, setup.and_then(|s| s.find(d)));
            // What it was kept as before you stopped keeping it, if anything.
            let before = if status == Status::NotKept { setup.and_then(|s| s.find_before(d)) } else { None };
            let lit = keeplight_core::lit_by::lit_by(d, setup.is_some_and(|s| s.find(d).is_some()), running);
            let capture = Kept::capture(d);
            let dark = is_dark(d);
            // A dark device can be kept too, but the row asks first.
            let can_keep = setup.is_some() && (capture.is_ok() || matches!(capture, Err(KeepError::AllOff)));
            let status_text = match (&capture, status) {
                (Err(KeepError::AllOff), Status::NotKept) => {
                    "OpenRGB reports every light as off, though another app may be lighting it. Keeping it keeps \
                     it off."
                        .to_string()
                }
                (Err(e), Status::NotKept) => e.to_string(),
                _ => status_text,
            };
            // On its own lighting, the colors OpenRGB reports are stale. The
            // "Lit by" line names the mode, so this only says what it means.
            let status_text = match (d.own_lighting(), status) {
                (Some(_), Status::NotKept) if status_text.is_empty() => {
                    "Keeplight can't see the colors it shows.".to_string()
                }
                _ => status_text,
            };
            let status_text = match before {
                Some(k) if status_text.is_empty() => format!("Kept before as {}.", describe(k)),
                Some(k) => format!("{status_text} Kept before as {}.", describe(k)),
                None => status_text,
            };
            Row {
                name_key: d.name.clone(),
                location_key: d.location.clone(),
                name,
                product: d.name.clone(),
                meta,
                zones,
                note,
                swatch: d.lit_color().map(|c| (c.r, c.g, c.b)),
                own_lighting: d.own_lighting().is_some(),
                lit_by: lit.describe(),
                lit_by_warn: lit.is_conflict(),
                status,
                status_text,
                can_keep,
                dark,
                has_before: before.is_some(),
            }
        })
        .collect()
}

/// A kept setup in a few words, such as "Chase Fade in #ff0000".
fn describe(k: &Kept) -> String {
    match k.main_color() {
        Some(c) => format!("{} in {}", k.mode.name, c.hex()),
        None => k.mode.name.clone(),
    }
}

fn status(d: &Device, kept: Option<&Kept>) -> (Status, String) {
    let Some(k) = kept else { return (Status::NotKept, String::new()) };
    let what = describe(k);
    match k.fit(d) {
        Fit::Same => (Status::Kept, format!("Kept: {what}.")),
        Fit::Differs => (Status::Differs, format!("Showing something other than what you kept ({what}).")),
        Fit::Incompatible(reason) => {
            (Status::Changed, format!("Kept setup no longer fits: {reason}. Keep it again to update it."))
        }
    }
}

fn message(chip: &str, heading: &str, message: &str) -> View {
    View {
        screen: Screen::Message,
        chip: chip.to_string(),
        chip_warn: true,
        heading: heading.to_string(),
        message: message.to_string(),
        summary: String::new(),
        notice: String::new(),
        notice_warn: false,
        rows: Vec::new(),
        sign_in: false,
        sign_in_note: String::new(),
        last_run: String::new(),
        tray_icon: false,
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

#[cfg(test)]
mod tests {
    use keeplight_core::device::{Mode, Rgb, Zone, color_mode};

    use super::*;

    fn device(kind: Kind, name: &str, mode: &str, colors: Vec<Rgb>, saves: bool) -> Device {
        Device {
            index: 0,
            kind,
            name: name.into(),
            vendor: String::new(),
            location: format!("{name} location"),
            modes: vec![Mode {
                name: mode.into(),
                flags: if saves { 1 << 8 } else { 0 },
                color_mode: color_mode::PER_LED,
                ..Mode::default()
            }],
            active_mode: Some(0),
            zones: vec![Zone { name: "Main".into(), led_count: colors.len() as u32, matrix: None }],
            colors,
        }
    }

    const RED: Rgb = Rgb { r: 255, g: 0, b: 0 };
    const BLUE: Rgb = Rgb { r: 0, g: 0, b: 255 };

    fn snap(devices: Vec<Device>) -> Result<Snapshot, String> {
        Ok(Snapshot { server_protocol: 6, devices })
    }

    fn empty() -> SetupView {
        Ok(Setup::default())
    }

    #[test]
    fn unreachable_server_explains_where_it_looked() {
        let v = build(&Err("refused".into()), &empty(), "127.0.0.1:6742", None, &[]);
        assert_eq!(v.screen, Screen::Message);
        assert!(v.chip_warn);
        assert!(v.message.contains("127.0.0.1:6742"));
        assert!(v.message.contains("refused"));
    }

    #[test]
    fn no_devices_is_its_own_message() {
        let v = build(&snap(vec![]), &empty(), "a", None, &[]);
        assert_eq!(v.screen, Screen::Message);
        assert!(v.heading.contains("hasn't found any lights"));
    }

    #[test]
    fn devices_get_plain_names_numbered_by_kind() {
        let s = snap(vec![
            device(Kind::Ram, "ENE DRAM", "Chase Fade", vec![RED; 8], false),
            device(Kind::Ram, "ENE DRAM", "Chase Fade", vec![RED; 8], false),
            device(Kind::GraphicsCard, "EVGA RTX", "Direct", vec![RED; 4], true),
            device(Kind::Keyboard, "Apex", "Direct", vec![Rgb::default(); 2], false),
        ]);
        let v = build(&s, &empty(), "a", None, &[]);
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
        let v = build(&snap(vec![device(Kind::Keyboard, "Apex", "Direct", colors, false)]), &empty(), "a", None, &[]);
        assert_eq!(v.rows[0].swatch, Some((255, 0, 0)));
        assert!(v.rows[0].can_keep);
    }

    #[test]
    fn several_zones_are_listed_with_their_counts() {
        let mut d = device(Kind::Motherboard, "Board", "Direct", vec![RED; 6], true);
        d.zones = vec![
            Zone { name: "Board".into(), led_count: 4, matrix: None },
            Zone { name: "Header 1".into(), led_count: 2, matrix: None },
        ];
        let v = build(&snap(vec![d]), &empty(), "a", None, &[]);
        assert_eq!(v.rows[0].zones, "Board 4 · Header 1 2");
        assert_eq!(v.summary, "1 device · 6 lights");
    }

    #[test]
    fn rows_show_how_each_device_stands_against_what_was_kept() {
        let red = device(Kind::Ram, "ENE DRAM", "Chase Fade", vec![RED; 8], false);
        let mut setup = Setup::default();
        setup.keep(Kept::capture(&red).unwrap());
        let gpu = device(Kind::GraphicsCard, "EVGA RTX", "Direct", vec![RED; 4], true);
        let mut blue = red.clone();
        blue.colors = vec![BLUE; 8];

        let v = build(&snap(vec![red.clone(), gpu.clone()]), &Ok(setup.clone()), "a", None, &[]);
        assert_eq!(v.summary, "2 devices · 12 lights · 1 kept");
        assert_eq!(v.rows[0].status, Status::Kept);
        assert_eq!(v.rows[0].status_text, "Kept: Chase Fade in #ff0000.");
        assert_eq!(v.rows[1].status, Status::NotKept);
        assert!(v.rows[1].can_keep);

        let v = build(&snap(vec![blue]), &Ok(setup.clone()), "a", None, &[]);
        assert_eq!(v.rows[0].status, Status::Differs);

        let mut more = red.clone();
        more.colors.push(RED);
        let v = build(&snap(vec![more]), &Ok(setup), "a", None, &[]);
        assert_eq!(v.rows[0].status, Status::Changed);
        assert!(v.rows[0].status_text.contains("9 lights"));
    }

    #[test]
    fn a_dark_device_says_why_it_cant_be_kept() {
        let dark = device(Kind::Keyboard, "Apex", "Direct", vec![Rgb::default(); 4], false);
        let v = build(&snap(vec![dark]), &empty(), "a", None, &[]);
        assert!(v.rows[0].can_keep, "it can be kept, after the row asks");
        assert!(v.rows[0].dark);
        assert!(v.rows[0].status_text.contains("another app may be lighting it"));
    }

    #[test]
    fn an_unusable_setup_turns_keeping_off_and_says_so() {
        let red = device(Kind::Ram, "ENE DRAM", "Chase Fade", vec![RED; 8], false);
        let notice = Notice { text: "Kept RAM stick 1.".into(), warn: false };
        let v = build(&snap(vec![red]), &Err("it isn't valid JSON".into()), "a", Some(&notice), &[]);
        assert!(v.notice_warn);
        assert!(v.notice.contains("it isn't valid JSON"));
        assert!(!v.rows[0].can_keep);
    }

    #[test]
    fn sign_in_says_what_it_will_do() {
        let exe = r#""C:\Keeplight\keeplight.exe" --keep"#;
        let (on, note) = sign_in(None, Some(exe), 2);
        assert!(!on && note.starts_with("Off: kept devices"));
        assert!(sign_in(None, Some(exe), 0).1.contains("Keep a device first"));
        let (on, note) = sign_in(Some(exe), Some(exe), 2);
        assert!(on && note.contains("after sleep"));
        assert!(sign_in(Some(exe), Some(exe), 0).1.contains("nothing is kept yet"));
        let other = r#""C:\dev\target\debug\keeplight.exe" --keep"#;
        let (on, note) = sign_in(Some(other), Some(exe), 2);
        assert!(on && note.contains("another copy") && note.contains("debug"));
        assert!(sign_in(Some(&exe.to_uppercase()), Some(exe), 1).1.contains("after sleep"), "paths ignore case");
    }

    #[test]
    fn a_device_stopped_earlier_offers_its_earlier_setup() {
        let red = device(Kind::Keyboard, "Apex", "Direct", vec![RED; 4], false);
        let mut setup = Setup::default();
        setup.before.push(Kept::capture(&red).unwrap());
        let mut now = red.clone();
        now.modes[0].name = "Onboard".into();
        now.modes[0].color_mode = keeplight_core::device::color_mode::NONE;
        let v = build(&snap(vec![now]), &Ok(setup), "a", None, &[]);
        assert_eq!(v.rows[0].status, Status::NotKept);
        assert!(v.rows[0].has_before);
        assert_eq!(v.rows[0].status_text, "Keeplight can't see the colors it shows. Kept before as Direct in #ff0000.");
        assert!(v.rows[0].own_lighting);
        assert_eq!(v.rows[0].swatch, None);
        assert!(v.rows[0].can_keep, "it can still be kept as it is now");
    }

    #[test]
    fn each_row_says_what_probably_lights_it() {
        let ram = device(Kind::Ram, "ENE DRAM", "Chase Fade", vec![RED; 8], false);
        let mut setup = Setup::default();
        setup.keep(Kept::capture(&ram).unwrap());
        let mut kb = device(Kind::Keyboard, "SteelSeries Apex", "Onboard", vec![RED; 4], false);
        kb.modes[0].color_mode = keeplight_core::device::color_mode::NONE;
        let v = build(&snap(vec![ram.clone(), kb]), &Ok(setup.clone()), "a", None, &[]);
        assert_eq!(v.rows[0].lit_by, "Keeplight, which keeps it");
        assert_eq!(v.rows[1].lit_by, "its own memory (Onboard)");
        assert!(!v.rows[0].lit_by_warn);

        let mut asus_ram = ram.clone();
        asus_ram.vendor = "ENE".into();
        let v = build(&snap(vec![asus_ram]), &Ok(setup), "a", None, &["armourycrate.service.exe".into()]);
        assert!(v.rows[0].lit_by_warn, "Armoury Crate may fight Keeplight over kept RAM");
    }

    #[test]
    fn the_last_action_shows_as_a_notice() {
        let notice = Notice { text: "Put back RAM stick 1.".into(), warn: false };
        let v = build(&snap(vec![]), &empty(), "a", Some(&notice), &[]);
        assert_eq!(v.notice, "Put back RAM stick 1.");
        assert!(!v.notice_warn);
    }
}
