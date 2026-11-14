//! A best guess at what is lighting a device, for the window to show.
//!
//! Nothing can say for certain which program last wrote to a USB or SMBus
//! device: Windows doesn't expose it, and lighting apps work through their own
//! drivers and services. The guess combines three things Keeplight can see:
//! whether it keeps the device, the mode OpenRGB reports, and which lighting
//! apps are running, matched to the device's maker. Wherever it is a guess,
//! the wording says "probably".

use crate::device::{Device, color_mode};

/// What is lighting a device, as far as Keeplight can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LitBy {
    /// Keeplight keeps it. `rival` names a running app that may change it.
    Keeplight { rival: Option<&'static str> },
    /// Probably this running app, made for the device's maker (or for every
    /// maker, such as SignalRGB).
    App(&'static str),
    /// Its own memory: the lighting saved on the device, such as a
    /// keyboard's Onboard mode.
    OwnMemory(String),
    /// A lighting effect built into the device, such as a rainbow.
    OwnEffect(String),
    /// OpenRGB is feeding it colors directly (Direct mode), set from
    /// OpenRGB's window or another OpenRGB client.
    OpenRgbDirect,
    /// A mode set through OpenRGB, such as the RAM's Chase Fade.
    OpenRgbMode(String),
}

impl LitBy {
    /// The words after "Lit by: ".
    pub fn describe(&self) -> String {
        match self {
            LitBy::Keeplight { rival: None } => "Keeplight, which keeps it".into(),
            LitBy::Keeplight { rival: Some(app) } => {
                format!("Keeplight, which keeps it, though {app} is running and may change it")
            }
            LitBy::App(app) => format!("probably {app}, which is running"),
            LitBy::OwnMemory(mode) => format!("its own memory ({mode})"),
            LitBy::OwnEffect(mode) => format!("its own built-in {mode} effect"),
            LitBy::OpenRgbDirect => "OpenRGB, as last set there (Direct)".into(),
            LitBy::OpenRgbMode(mode) => format!("the {mode} mode, as last set in OpenRGB"),
        }
    }

    /// Whether this is worth a warning: another app may fight Keeplight.
    pub fn is_conflict(&self) -> bool {
        matches!(self, LitBy::Keeplight { rival: Some(_) })
    }
}

/// A maker's lighting app: the words that identify the maker's devices in
/// OpenRGB's vendor or name, and the app's process names (lower case, with
/// `.exe`). An empty maker list means it drives every maker's devices.
struct KnownApp {
    name: &'static str,
    makers: &'static [&'static str],
    processes: &'static [&'static str],
}

const KNOWN_APPS: &[KnownApp] = &[
    KnownApp { name: "SignalRGB", makers: &[], processes: &["signalrgb.exe"] },
    KnownApp {
        name: "SteelSeries GG",
        makers: &["steelseries"],
        processes: &["steelseriesgg.exe", "steelseriesengine.exe", "steelseriesprism.exe"],
    },
    KnownApp { name: "Logitech G HUB", makers: &["logitech"], processes: &["lghub.exe", "lghub_agent.exe"] },
    // ASUS's AURA lighting also drives the ENE chips on RAM like this PC's.
    KnownApp {
        name: "Armoury Crate",
        makers: &["asus", "ene"],
        processes: &["armourycrate.service.exe", "armourycrate.exe", "lightingservice.exe"],
    },
    KnownApp { name: "Corsair iCUE", makers: &["corsair"], processes: &["icue.exe"] },
    KnownApp {
        name: "Razer Synapse",
        makers: &["razer"],
        processes: &["razer synapse 3.exe", "razer synapse service.exe", "razercentralservice.exe"],
    },
    KnownApp { name: "EVGA Precision X1", makers: &["evga"], processes: &["precisionx_x64.exe", "precision x1.exe"] },
    KnownApp { name: "NZXT CAM", makers: &["nzxt"], processes: &["nzxt cam.exe"] },
    KnownApp { name: "Gigabyte RGB Fusion", makers: &["gigabyte", "aorus"], processes: &["rgbfusion.exe"] },
];

/// The running lighting app that would light `device`, if any. `running`
/// holds process names in lower case, such as "lghub_agent.exe".
fn rival(device: &Device, running: &[String]) -> Option<&'static str> {
    let maker = format!("{} {}", device.vendor, device.name).to_lowercase();
    KNOWN_APPS
        .iter()
        .filter(|app| app.makers.is_empty() || app.makers.iter().any(|m| maker.contains(m)))
        .find(|app| app.processes.iter().any(|p| running.iter().any(|r| r == p)))
        .map(|app| app.name)
}

/// What is probably lighting `device`. `kept` says whether Keeplight keeps
/// it; `running` holds the names of running processes, in lower case.
pub fn lit_by(device: &Device, kept: bool, running: &[String]) -> LitBy {
    let app = rival(device, running);
    if kept {
        return LitBy::Keeplight { rival: app };
    }
    if let Some(app) = app {
        return LitBy::App(app);
    }
    let Some(mode) = device.active() else { return LitBy::OpenRgbDirect };
    let name = mode.name.clone();
    match mode.color_mode {
        _ if name.eq_ignore_ascii_case("direct") => LitBy::OpenRgbDirect,
        _ if name.eq_ignore_ascii_case("onboard") || name.eq_ignore_ascii_case("hardware") => LitBy::OwnMemory(name),
        color_mode::NONE | color_mode::RANDOM if !name.eq_ignore_ascii_case("off") => LitBy::OwnEffect(name),
        _ => LitBy::OpenRgbMode(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Rgb;
    use crate::testing::{self, RED};

    fn running(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn onboard_keyboard() -> Device {
        let mut kb = testing::keyboard(RED);
        kb.vendor = "SteelSeries".into();
        kb.active_mode = Some(1);
        kb
    }

    #[test]
    fn the_keyboard_on_onboard_with_no_app_is_its_own_memory() {
        let kb = onboard_keyboard();
        let lit = lit_by(&kb, false, &running(&["openrgb.exe", "lghub_agent.exe"]));
        assert_eq!(lit, LitBy::OwnMemory("Onboard".into()));
        assert_eq!(lit.describe(), "its own memory (Onboard)");
    }

    #[test]
    fn the_makers_app_running_is_the_likely_one() {
        let lit = lit_by(&onboard_keyboard(), false, &running(&["steelseriesgg.exe"]));
        assert_eq!(lit.describe(), "probably SteelSeries GG, which is running");
        // Another maker's app doesn't count.
        assert_eq!(lit_by(&onboard_keyboard(), false, &running(&["icue.exe"])), LitBy::OwnMemory("Onboard".into()));
    }

    #[test]
    fn signalrgb_drives_every_maker() {
        assert_eq!(lit_by(&testing::ram("SMBus"), false, &running(&["signalrgb.exe"])), LitBy::App("SignalRGB"));
    }

    #[test]
    fn armoury_crate_counts_for_ene_ram() {
        let lit = lit_by(&testing::ram("SMBus"), true, &running(&["armourycrate.service.exe"]));
        assert_eq!(lit, LitBy::Keeplight { rival: Some("Armoury Crate") });
        assert!(lit.is_conflict());
        assert!(lit.describe().contains("may change it"));
    }

    #[test]
    fn kept_devices_are_lit_by_keeplight() {
        let lit = lit_by(&testing::ram("SMBus"), true, &[]);
        assert_eq!(lit.describe(), "Keeplight, which keeps it");
        assert!(!lit.is_conflict());
    }

    #[test]
    fn openrgb_modes_and_built_in_effects() {
        assert_eq!(lit_by(&testing::keyboard(RED), false, &[]), LitBy::OpenRgbDirect);
        assert_eq!(lit_by(&testing::ram("SMBus"), false, &[]), LitBy::OpenRgbMode("Chase Fade".into()));
        let mut rainbow = testing::ram("SMBus");
        rainbow.active_mode = Some(1);
        assert_eq!(lit_by(&rainbow, false, &[]), LitBy::OwnEffect("Rainbow".into()));
        let mut off = testing::board();
        off.modes[1].colors = vec![Rgb::default()];
        assert_eq!(lit_by(&off, false, &[]), LitBy::OpenRgbMode("Static".into()));
    }
}
