//! The keeper's setup: which devices you chose to keep and what they showed
//! when you did, saved in Keeplight's own file, and putting that back.
//!
//! The setup holds only devices you chose. A device is matched by its product
//! name and location (OpenRGB's index can change between scans), and is only
//! ever sent its own saved mode and colors.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::device::{Device, Mode, Rgb, color_mode};
use crate::sdk::{Client, SdkError};

/// The setup file's format version. A file from a newer Keeplight is left
/// untouched rather than read wrongly.
pub const SETUP_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setup {
    pub version: u32,
    pub devices: Vec<Kept>,
}

impl Default for Setup {
    fn default() -> Self {
        Setup { version: SETUP_VERSION, devices: Vec::new() }
    }
}

/// One device as it was when you chose to keep it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// The product name OpenRGB reports, such as "ENE DRAM".
    pub name: String,
    /// Where OpenRGB found it, such as "I2C: ... address 0x71".
    pub location: String,
    pub led_count: usize,
    /// The mode it was in, with every setting.
    pub mode: Mode,
    /// One color per light, for a mode that colors each light itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<Rgb>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeepError {
    #[error("every light on it is off, so there is no color to keep. Set a color in OpenRGB first.")]
    AllOff,
    #[error("OpenRGB doesn't say which mode it's in, so there is nothing to keep yet.")]
    NoMode,
}

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("Keeplight couldn't read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("{path} isn't a setup Keeplight can read, so it was left as it is: {detail}")]
    Corrupt { path: PathBuf, detail: String },
    #[error("{path} was written by a newer Keeplight (format {found}), so it was left as it is.")]
    Newer { path: PathBuf, found: u32 },
    #[error("Keeplight couldn't save {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
}

/// How a device compares with what was kept for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fit {
    /// Showing what was kept.
    Same,
    /// Showing something else; putting it back can fix that.
    Differs,
    /// The device changed in a way the kept setup no longer fits, such as a
    /// different light count. Keeplight won't send it anything.
    Incompatible(String),
}

/// What happened when a device was put back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Sent, and read back as kept.
    Held,
    /// Sent, but the device read back as something else.
    NotHeld,
    /// Nothing was sent, for the reason given.
    Skipped(String),
}

impl Kept {
    /// What `device` shows right now, ready to keep. Refuses a device whose
    /// lights are all off; keeping that would be keeping it dark.
    pub fn capture(device: &Device) -> Result<Kept, KeepError> {
        let mode = device.active().ok_or(KeepError::NoMode)?;
        let lit = match mode.color_mode {
            color_mode::PER_LED => device.colors.iter().any(|c| !c.is_black()),
            color_mode::MODE_SPECIFIC => mode.colors.iter().any(|c| !c.is_black()),
            // An effect with its own colors lights up by itself.
            _ => true,
        };
        if !lit {
            return Err(KeepError::AllOff);
        }
        let colors = if mode.color_mode == color_mode::PER_LED { device.colors.clone() } else { Vec::new() };
        Ok(Kept {
            name: device.name.clone(),
            location: device.location.clone(),
            led_count: device.led_count(),
            mode: mode.clone(),
            colors,
        })
    }

    pub fn is_for(&self, device: &Device) -> bool {
        self.name == device.name && self.location == device.location
    }

    pub fn fit(&self, device: &Device) -> Fit {
        if device.led_count() != self.led_count {
            return Fit::Incompatible(format!(
                "it now reports {} lights, not the {} it had when you kept it",
                device.led_count(),
                self.led_count
            ));
        }
        if device.mode_index(&self.mode.name).is_none() {
            return Fit::Incompatible(format!("it no longer has the {} mode", self.mode.name));
        }
        let Some(active) = device.active() else { return Fit::Differs };
        // The settings a person can change within a mode. The min and max
        // fields describe what the device can do, so they are left out.
        let settings = |m: &Mode| (m.speed, m.brightness, m.direction, m.color_mode);
        let same = active.name == self.mode.name
            && settings(active) == settings(&self.mode)
            && match self.mode.color_mode {
                color_mode::PER_LED => device.colors == self.colors,
                color_mode::MODE_SPECIFIC => active.colors == self.mode.colors,
                _ => true,
            };
        if same { Fit::Same } else { Fit::Differs }
    }

    /// The color most of the kept lights show, for a swatch or a sentence.
    pub fn main_color(&self) -> Option<Rgb> {
        let shown = if self.colors.is_empty() { &self.mode.colors } else { &self.colors };
        let mut counts: Vec<(Rgb, usize)> = Vec::new();
        for c in shown.iter().filter(|c| !c.is_black()) {
            match counts.iter_mut().find(|(k, _)| k == c) {
                Some((_, n)) => *n += 1,
                None => counts.push((*c, 1)),
            }
        }
        counts.into_iter().rev().max_by_key(|&(_, n)| n).map(|(c, _)| c)
    }
}

impl Setup {
    /// Reads the setup at `path`. A missing file is an empty setup; a file
    /// that can't be read is an error, and the caller must not save over it.
    pub fn load(path: &Path) -> Result<Setup, SetupError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Setup::default()),
            Err(source) => return Err(SetupError::Read { path: path.into(), source }),
        };
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        let corrupt = |e: serde_json::Error| SetupError::Corrupt { path: path.into(), detail: e.to_string() };
        let found = serde_json::from_str::<Version>(&text).map_err(corrupt)?.version;
        if found > SETUP_VERSION {
            return Err(SetupError::Newer { path: path.into(), found });
        }
        serde_json::from_str(&text).map_err(corrupt)
    }

    /// Writes the setup to `path`, through a temporary file and a rename so a
    /// crash mid-save leaves the old setup whole.
    pub fn save(&self, path: &Path) -> Result<(), SetupError> {
        let write = |source| SetupError::Write { path: path.into(), source };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(write)?;
        }
        let mut text = serde_json::to_string_pretty(self).map_err(|e| write(io::Error::other(e)))?;
        text.push('\n');
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(write)?;
        std::fs::rename(&tmp, path).map_err(write)
    }

    pub fn find(&self, device: &Device) -> Option<&Kept> {
        self.devices.iter().find(|k| k.is_for(device))
    }

    /// Adds `kept`, replacing what was kept for the same device before.
    pub fn keep(&mut self, kept: Kept) {
        match self.devices.iter_mut().find(|k| k.name == kept.name && k.location == kept.location) {
            Some(existing) => *existing = kept,
            None => self.devices.push(kept),
        }
    }
}

/// Sends `device` its kept mode, and its kept colors when the mode colors each
/// light, then reads it back. Sends nothing when the kept setup no longer
/// fits the device.
pub fn put_back(client: &mut Client, kept: &Kept, device: &Device) -> Result<Outcome, SdkError> {
    if !kept.is_for(device) {
        return Ok(Outcome::Skipped("that is a different device".into()));
    }
    if let Fit::Incompatible(reason) = kept.fit(device) {
        return Ok(Outcome::Skipped(reason));
    }
    let Some(index) = device.mode_index(&kept.mode.name) else {
        return Ok(Outcome::Skipped(format!("it no longer has the {} mode", kept.mode.name)));
    };
    client.update_mode(device.index, index, &kept.mode)?;
    if kept.mode.color_mode == color_mode::PER_LED {
        client.update_leds(device.index, &kept.colors)?;
    }
    let after = client.controller(device.index)?;
    Ok(if kept.is_for(&after) && kept.fit(&after) == Fit::Same { Outcome::Held } else { Outcome::NotHeld })
}

/// Why a keep or put-back request from a front end didn't happen.
#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    #[error(transparent)]
    Sdk(#[from] SdkError),
    #[error(transparent)]
    Setup(#[from] SetupError),
    #[error("Keeplight can't keep this device: {0}")]
    Keep(#[from] KeepError),
    #[error("OpenRGB no longer lists that device. It may have been unplugged or renamed.")]
    NotFound,
    #[error("Nothing is kept for that device yet.")]
    NotKept,
}

/// Finds a device by product name and location in a fresh read.
fn find_device(client: &mut Client, name: &str, location: &str) -> Result<Device, ActionError> {
    let snapshot = client.snapshot()?;
    snapshot.devices.into_iter().find(|d| d.name == name && d.location == location).ok_or(ActionError::NotFound)
}

/// Keeps what the named device shows right now: reads it fresh from OpenRGB
/// at `address` and saves it into the setup at `setup_path`. Sends nothing
/// that changes a light.
pub fn keep_now(address: &str, setup_path: &Path, name: &str, location: &str) -> Result<Kept, ActionError> {
    // Read the setup first, so an unreadable one is never saved over.
    let mut setup = Setup::load(setup_path)?;
    let mut client = Client::connect(address)?;
    let kept = Kept::capture(&find_device(&mut client, name, location)?)?;
    setup.keep(kept.clone());
    setup.save(setup_path)?;
    Ok(kept)
}

/// Puts the named device back to what is kept for it (see [`put_back`]).
pub fn put_back_now(address: &str, setup_path: &Path, name: &str, location: &str) -> Result<Outcome, ActionError> {
    let setup = Setup::load(setup_path)?;
    let mut client = Client::connect(address)?;
    let device = find_device(&mut client, name, location)?;
    let kept = setup.find(&device).ok_or(ActionError::NotKept)?;
    Ok(put_back(&mut client, kept, &device)?)
}

/// One kept device's result in [`put_back_all`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub name: String,
    pub location: String,
    /// What happened, or why it couldn't be tried, such as the device not
    /// being listed.
    pub result: Result<Outcome, String>,
}

/// The result of putting back every kept device.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Run {
    pub reports: Vec<Report>,
    /// How long it waited for OpenRGB to list every kept device.
    pub waited: Duration,
}

impl Run {
    pub fn held(&self) -> usize {
        self.reports.iter().filter(|r| r.result == Ok(Outcome::Held)).count()
    }
}

/// Puts back every device in the setup at `setup_path`, as the keeper does at
/// sign-in. OpenRGB may still be starting, or still finding devices, so it
/// retries every `poll` until it lists every kept device or `wait` runs out,
/// then puts back what it found. Devices that are not kept are never sent
/// anything. An empty setup sends nothing and connects to nothing.
pub fn put_back_all(address: &str, setup_path: &Path, wait: Duration, poll: Duration) -> Result<Run, ActionError> {
    let setup = Setup::load(setup_path)?;
    if setup.devices.is_empty() {
        return Ok(Run::default());
    }
    let start = Instant::now();
    let (mut client, devices) = loop {
        let attempt = Client::connect(address).and_then(|mut c| c.snapshot().map(|s| (c, s.devices)));
        let out_of_time = start.elapsed() + poll > wait;
        match attempt {
            Ok((c, devices)) if out_of_time || setup.devices.iter().all(|k| devices.iter().any(|d| k.is_for(d))) => {
                break (c, devices);
            }
            Err(e) if out_of_time => return Err(e.into()),
            _ => std::thread::sleep(poll),
        }
    };
    let waited = start.elapsed();
    let mut reports = Vec::new();
    for kept in &setup.devices {
        let result = match devices.iter().find(|d| kept.is_for(d)) {
            Some(device) => put_back(&mut client, kept, device).map_err(|e| e.to_string()),
            None => Err("OpenRGB doesn't list it".to_string()),
        };
        reports.push(Report { name: kept.name.clone(), location: kept.location.clone(), result });
    }
    Ok(Run { reports, waited })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::wire::packet;
    use crate::testing::{self, FakeServer, RED};

    const BLUE: Rgb = Rgb { r: 0, g: 0, b: 255 };

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("keeplight-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("setup.json")
    }

    /// RAM that lost power: back on its own rainbow.
    fn rainbow_ram(location: &str) -> Device {
        let mut d = testing::ram(location);
        d.active_mode = Some(1);
        d
    }

    #[test]
    fn captures_a_mode_with_its_own_colors() {
        let k = Kept::capture(&testing::board()).unwrap();
        assert_eq!(k.mode.name, "Static");
        assert_eq!(k.mode.colors, [RED]);
        assert!(k.colors.is_empty(), "per-light colors aren't kept for this mode");
        assert_eq!(k.led_count, 4);
        assert_eq!(k.main_color(), Some(RED));
    }

    #[test]
    fn captures_the_ram_effect_with_its_colors() {
        let k = Kept::capture(&testing::ram("SMBus 0x71")).unwrap();
        assert_eq!(k.mode.name, "Chase Fade");
        assert_eq!(k.colors, vec![RED; 8]);
    }

    #[test]
    fn captures_per_light_colors() {
        let mut kb = testing::keyboard(RED);
        kb.colors[0] = BLUE;
        let k = Kept::capture(&kb).unwrap();
        assert_eq!(k.colors.len(), 6);
        assert_eq!(k.colors[0], BLUE);
        assert_eq!(k.main_color(), Some(RED));
    }

    #[test]
    fn refuses_to_keep_a_dark_device() {
        assert_eq!(Kept::capture(&testing::keyboard(Rgb::default())), Err(KeepError::AllOff));
        let mut board = testing::board();
        board.modes[1].colors = vec![Rgb::default()];
        assert_eq!(Kept::capture(&board), Err(KeepError::AllOff));
        board.active_mode = None;
        assert_eq!(Kept::capture(&board), Err(KeepError::NoMode));
        let mut off = testing::ram("SMBus");
        off.colors = vec![Rgb::default(); 8];
        assert_eq!(Kept::capture(&off), Err(KeepError::AllOff));
    }

    #[test]
    fn fit_compares_what_matters_for_the_mode() {
        let ram = testing::ram("SMBus");
        let k = Kept::capture(&ram).unwrap();
        assert_eq!(k.fit(&ram), Fit::Same);
        assert_eq!(k.fit(&rainbow_ram("SMBus")), Fit::Differs);
        let mut blue_stick = ram.clone();
        blue_stick.colors[0] = BLUE;
        assert_eq!(k.fit(&blue_stick), Fit::Differs);
        let board = testing::board();
        let kb_board = Kept::capture(&board).unwrap();
        let mut blue = board.clone();
        blue.modes[1].colors = vec![BLUE];
        assert_eq!(kb_board.fit(&blue), Fit::Differs);
        assert_eq!(kb_board.fit(&board), Fit::Same);
        let mut fewer = ram.clone();
        fewer.colors.pop();
        assert!(matches!(k.fit(&fewer), Fit::Incompatible(_)));

        for change in [
            |m: &mut Mode| m.speed += 1,
            |m: &mut Mode| m.brightness += 1,
            |m: &mut Mode| m.direction += 1,
            |m: &mut Mode| m.color_mode = color_mode::NONE,
        ] {
            let mut tweaked = ram.clone();
            change(&mut tweaked.modes[2]);
            assert_eq!(k.fit(&tweaked), Fit::Differs, "a changed setting in the same mode differs");
        }
        let mut wider = ram.clone();
        wider.modes[2].speed_max += 1;
        assert_eq!(k.fit(&wider), Fit::Same, "what the device can do isn't a setting");

        let kb = testing::keyboard(RED);
        let k = Kept::capture(&kb).unwrap();
        let mut one_key = kb.clone();
        one_key.colors[5] = BLUE;
        assert_eq!(k.fit(&one_key), Fit::Differs);
    }

    #[test]
    fn setup_saves_loads_and_replaces_by_device() {
        let path = temp_path("round-trip");
        assert_eq!(Setup::load(&path).unwrap(), Setup::default());

        let mut setup = Setup::default();
        setup.keep(Kept::capture(&testing::ram("SMBus 0x71")).unwrap());
        setup.keep(Kept::capture(&testing::keyboard(RED)).unwrap());
        setup.keep(Kept::capture(&testing::keyboard(BLUE)).unwrap());
        assert_eq!(setup.devices.len(), 2, "keeping a device again replaces it");
        setup.save(&path).unwrap();

        let back = Setup::load(&path).unwrap();
        assert_eq!(back, setup);
        assert_eq!(back.find(&testing::keyboard(RED)).unwrap().colors, vec![BLUE; 6]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"#ff0000\""), "colors are saved as #rrggbb");
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn unreadable_or_newer_setups_are_errors() {
        let path = temp_path("bad");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(matches!(Setup::load(&path), Err(SetupError::Corrupt { .. })));
        std::fs::write(&path, r#"{ "version": 99, "devices": [] }"#).unwrap();
        assert!(matches!(Setup::load(&path), Err(SetupError::Newer { found: 99, .. })));
    }

    #[test]
    fn putting_back_sends_only_that_device_its_own_setup() {
        let kept = Kept::capture(&testing::ram("SMBus 0x73")).unwrap();
        let server = FakeServer::start(vec![rainbow_ram("SMBus 0x71"), rainbow_ram("SMBus 0x73")]);
        let mut client = Client::connect(&server.address).unwrap();
        let device = client.controller(1).unwrap();
        assert_eq!(put_back(&mut client, &kept, &device).unwrap(), Outcome::Held);

        assert_eq!(server.device(1).active_mode_name(), Some("Chase Fade"));
        assert_eq!(server.device(0).active_mode_name(), Some("Rainbow"), "the other stick is untouched");
        let writes: Vec<_> = server
            .state
            .lock()
            .unwrap()
            .seen
            .iter()
            .filter(|(id, _)| ![packet::REQUEST_PROTOCOL_VERSION, packet::SET_CLIENT_NAME].contains(id))
            .filter(|(id, _)| ![packet::REQUEST_CONTROLLER_COUNT, packet::REQUEST_CONTROLLER_DATA].contains(id))
            .copied()
            .collect();
        assert_eq!(writes, [(packet::UPDATE_MODE, 1), (packet::UPDATE_LEDS, 1)]);
    }

    #[test]
    fn a_mode_with_its_own_colors_needs_no_per_light_colors() {
        let kept = Kept::capture(&testing::board()).unwrap();
        let mut direct = testing::board();
        direct.active_mode = Some(0);
        let server = FakeServer::start(vec![direct]);
        let mut client = Client::connect(&server.address).unwrap();
        let device = client.controller(0).unwrap();
        assert_eq!(put_back(&mut client, &kept, &device).unwrap(), Outcome::Held);
        let ids = server.seen_ids();
        assert!(ids.contains(&packet::UPDATE_MODE) && !ids.contains(&packet::UPDATE_LEDS), "sent {ids:?}");
    }

    #[test]
    fn putting_back_per_light_colors() {
        let mut kb = testing::keyboard(RED);
        kb.colors[1] = BLUE;
        let kept = Kept::capture(&kb).unwrap();
        let server = FakeServer::start(vec![testing::keyboard(Rgb::default())]);
        let mut client = Client::connect(&server.address).unwrap();
        let device = client.controller(0).unwrap();
        assert_eq!(put_back(&mut client, &kept, &device).unwrap(), Outcome::Held);
        assert_eq!(server.device(0).colors, kb.colors);
    }

    #[test]
    fn putting_back_restores_a_changed_speed() {
        let kept = Kept::capture(&testing::ram("SMBus 0x71")).unwrap();
        let mut faster = testing::ram("SMBus 0x71");
        faster.modes[2].speed = 9;
        let server = FakeServer::start(vec![faster]);
        let mut client = Client::connect(&server.address).unwrap();
        let device = client.controller(0).unwrap();
        assert_eq!(kept.fit(&device), Fit::Differs);
        assert_eq!(put_back(&mut client, &kept, &device).unwrap(), Outcome::Held);
        assert_eq!(server.device(0).modes[2].speed, 0);
    }

    #[test]
    fn a_device_that_ignores_the_request_is_not_held() {
        let kept = Kept::capture(&testing::ram("SMBus 0x71")).unwrap();
        let server = FakeServer::start(vec![rainbow_ram("SMBus 0x71")]);
        server.state.lock().unwrap().stuck.push(0);
        let mut client = Client::connect(&server.address).unwrap();
        let device = client.controller(0).unwrap();
        assert_eq!(put_back(&mut client, &kept, &device).unwrap(), Outcome::NotHeld);
    }

    #[test]
    fn keep_now_and_put_back_now_work_by_name_and_location() {
        let path = temp_path("actions");
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71"), testing::keyboard(RED)]);
        let kept = keep_now(&server.address, &path, "ENE DRAM", "SMBus 0x71").unwrap();
        assert_eq!(kept.mode.name, "Chase Fade");
        assert_eq!(Setup::load(&path).unwrap().devices, [kept]);
        assert!(!server.seen_ids().contains(&packet::UPDATE_MODE), "keeping changes no light");

        server.state.lock().unwrap().devices[0].active_mode = Some(1);
        assert_eq!(put_back_now(&server.address, &path, "ENE DRAM", "SMBus 0x71").unwrap(), Outcome::Held);
        assert_eq!(server.device(0).active_mode_name(), Some("Chase Fade"));
        assert!(matches!(put_back_now(&server.address, &path, "Apex", "HID: keyboard"), Err(ActionError::NotKept)));
        assert!(matches!(keep_now(&server.address, &path, "Gone", "nowhere"), Err(ActionError::NotFound)));
    }

    #[test]
    fn keep_now_never_saves_over_an_unreadable_setup() {
        let path = temp_path("keep-corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        assert!(matches!(keep_now(&server.address, &path, "ENE DRAM", "SMBus 0x71"), Err(ActionError::Setup(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    const QUICK: Duration = Duration::from_millis(20);

    #[test]
    fn put_back_all_restores_every_kept_device_and_nothing_else() {
        let path = temp_path("all");
        let mut setup = Setup::default();
        setup.keep(Kept::capture(&testing::ram("SMBus 0x71")).unwrap());
        setup.keep(Kept::capture(&testing::ram("SMBus 0x73")).unwrap());
        setup.save(&path).unwrap();
        let server =
            FakeServer::start(vec![rainbow_ram("SMBus 0x71"), testing::keyboard(BLUE), rainbow_ram("SMBus 0x73")]);

        let run = put_back_all(&server.address, &path, Duration::from_secs(1), QUICK).unwrap();
        assert_eq!(run.reports.len(), 2);
        assert_eq!(run.held(), 2);
        assert_eq!(server.device(0).active_mode_name(), Some("Chase Fade"));
        assert_eq!(server.device(2).active_mode_name(), Some("Chase Fade"));
        assert_eq!(server.device(1), Device { index: 1, ..testing::keyboard(BLUE) }, "the keyboard isn't kept");
        let to_keyboard = server
            .state
            .lock()
            .unwrap()
            .seen
            .iter()
            .any(|&(id, dev)| dev == 1 && [packet::UPDATE_MODE, packet::UPDATE_LEDS].contains(&id));
        assert!(!to_keyboard);
    }

    #[test]
    fn put_back_all_waits_for_a_device_that_shows_up_late() {
        let path = temp_path("late");
        let mut setup = Setup::default();
        setup.keep(Kept::capture(&testing::ram("SMBus 0x73")).unwrap());
        setup.save(&path).unwrap();
        let server = FakeServer::start(vec![rainbow_ram("SMBus 0x71")]);
        let state = server.state.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            state.lock().unwrap().devices.push(Device { index: 1, ..rainbow_ram("SMBus 0x73") });
        });
        let run = put_back_all(&server.address, &path, Duration::from_secs(3), QUICK).unwrap();
        assert_eq!(run.held(), 1);
        assert!(run.waited >= Duration::from_millis(100));
    }

    #[test]
    fn put_back_all_reports_a_device_that_never_shows_up() {
        let path = temp_path("missing");
        let mut setup = Setup::default();
        setup.keep(Kept::capture(&testing::ram("SMBus 0x71")).unwrap());
        setup.keep(Kept::capture(&testing::ram("SMBus 0x75")).unwrap());
        setup.save(&path).unwrap();
        let server = FakeServer::start(vec![rainbow_ram("SMBus 0x71")]);
        let run = put_back_all(&server.address, &path, Duration::from_millis(200), QUICK).unwrap();
        assert_eq!(run.held(), 1);
        assert!(run.reports[1].result.as_ref().unwrap_err().contains("doesn't list"));
    }

    #[test]
    fn put_back_all_with_nothing_kept_contacts_nothing() {
        let path = temp_path("empty");
        // Port 9 on loopback is never OpenRGB; an attempt would fail the test.
        let run = put_back_all("127.0.0.1:9", &path, Duration::from_millis(50), QUICK).unwrap();
        assert!(run.reports.is_empty());
    }

    #[test]
    fn put_back_all_gives_up_when_openrgb_never_answers() {
        let path = temp_path("no-server");
        let mut setup = Setup::default();
        setup.keep(Kept::capture(&testing::ram("SMBus 0x71")).unwrap());
        setup.save(&path).unwrap();
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
        assert!(matches!(put_back_all(&closed, &path, Duration::from_millis(100), QUICK), Err(ActionError::Sdk(_))));
    }

    #[test]
    fn a_changed_device_is_skipped_without_sending_anything() {
        let kept = Kept::capture(&testing::keyboard(RED)).unwrap();
        let mut bigger = testing::keyboard(BLUE);
        bigger.colors.push(BLUE);
        let server = FakeServer::start(vec![bigger]);
        let mut client = Client::connect(&server.address).unwrap();
        let device = client.controller(0).unwrap();
        assert!(matches!(put_back(&mut client, &kept, &device).unwrap(), Outcome::Skipped(_)));
        let other = Kept { location: "elsewhere".into(), ..kept };
        assert!(matches!(put_back(&mut client, &other, &device).unwrap(), Outcome::Skipped(_)));
        let ids = server.seen_ids();
        assert!(!ids.contains(&packet::UPDATE_MODE) && !ids.contains(&packet::UPDATE_LEDS), "sent {ids:?}");
    }
}
