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
    /// What each device you stopped keeping was kept as, so "Keep it as
    /// before" can bring it back. Never put back by the keeper. Absent in
    /// setups written before this field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<Kept>,
}

impl Default for Setup {
    fn default() -> Self {
        Setup { version: SETUP_VERSION, devices: Vec::new(), before: Vec::new() }
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
    #[error("none of its modes takes a color, so it can only show its own lighting.")]
    NoColorMode,
    #[error("black turns every light off. To keep it dark, use Keep it dark instead.")]
    Black,
    #[error("OpenRGB doesn't say which mode it's in now, so Keep it as before couldn't bring it back afterwards.")]
    NoUndo,
}

/// What a new color on `device` replaces, for "Keep it as before": what is
/// kept for it, or what it shows when it isn't kept. A change that couldn't
/// be undone is refused.
pub fn undo_for(setup: &Setup, device: &Device) -> Result<Kept, KeepError> {
    match setup.find(device) {
        Some(k) => Ok(k.clone()),
        None => Kept::capture_dark(device).map_err(|_| KeepError::NoUndo),
    }
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

/// Whether every light on `device` is off: its "Off" mode, or a mode whose
/// colors are all black. An effect with colors of its own lights up by itself.
pub fn is_dark(device: &Device) -> bool {
    let Some(mode) = device.active() else { return false };
    if mode.name.eq_ignore_ascii_case("off") {
        return true;
    }
    match mode.color_mode {
        color_mode::PER_LED => device.colors.iter().all(|c| c.is_black()),
        color_mode::MODE_SPECIFIC => mode.colors.iter().all(|c| c.is_black()),
        _ => false,
    }
}

impl Kept {
    /// What `device` shows right now, ready to keep. Refuses a device whose
    /// lights are all off; keeping that would keep it dark, which only
    /// [`Kept::capture_dark`] does, after asking.
    pub fn capture(device: &Device) -> Result<Kept, KeepError> {
        if is_dark(device) {
            return Err(KeepError::AllOff);
        }
        Self::capture_dark(device)
    }

    /// What `device` shows right now, even when every light is off. For a
    /// front end that has asked first: keeping it means putting it back to
    /// dark after every sleep.
    pub fn capture_dark(device: &Device) -> Result<Kept, KeepError> {
        let mode = device.active().ok_or(KeepError::NoMode)?;
        let colors = if mode.color_mode == color_mode::PER_LED { device.colors.clone() } else { Vec::new() };
        Ok(Kept {
            name: device.name.clone(),
            location: device.location.clone(),
            led_count: device.led_count(),
            mode: mode.clone(),
            colors,
        })
    }

    /// `device` in `color`, ready to send and keep. A mode that takes colors
    /// keeps its effect and gets the new color (red Chase Fade becomes blue
    /// Chase Fade; owner decision 2026-11-13). A mode that doesn't, such as
    /// Onboard, a rainbow or Off, is swapped for the device's Static mode, or
    /// Direct when it has no Static.
    pub fn colored(device: &Device, color: Rgb) -> Result<Kept, KeepError> {
        if color.is_black() {
            return Err(KeepError::Black);
        }
        let takes_color = |m: &Mode| {
            !m.name.eq_ignore_ascii_case("off")
                && matches!(m.color_mode, color_mode::PER_LED | color_mode::MODE_SPECIFIC)
        };
        let named = |name: &str| device.modes.iter().find(|m| m.name.eq_ignore_ascii_case(name) && takes_color(m));
        let mode = device
            .active()
            .filter(|m| takes_color(m))
            .or_else(|| named("static"))
            .or_else(|| named("direct"))
            .ok_or(KeepError::NoColorMode)?;
        let mut mode = mode.clone();
        let colors = if mode.color_mode == color_mode::PER_LED {
            vec![color; device.led_count()]
        } else {
            let count = mode.colors.len().max(mode.colors_min as usize).max(1);
            mode.colors = vec![color; count];
            Vec::new()
        };
        Ok(Kept {
            name: device.name.clone(),
            location: device.location.clone(),
            led_count: device.led_count(),
            mode,
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

    /// What `device` was kept as before you stopped keeping it.
    pub fn find_before(&self, device: &Device) -> Option<&Kept> {
        self.before.iter().find(|k| k.is_for(device))
    }

    /// Adds `kept`, replacing what was kept for the same device before.
    pub fn keep(&mut self, kept: Kept) {
        // Keeping it again replaces any remembered earlier setup.
        self.before.retain(|k| !(k.name == kept.name && k.location == kept.location));
        match self.devices.iter_mut().find(|k| k.name == kept.name && k.location == kept.location) {
            Some(existing) => *existing = kept,
            None => self.devices.push(kept),
        }
    }

    /// Keeps `kept` and remembers `previous` (what was kept for the device, or
    /// what it showed) for "Keep it as before", so a change can be undone.
    pub fn keep_remembering(&mut self, kept: Kept, previous: Option<Kept>) {
        self.keep(kept);
        if let Some(previous) = previous {
            self.before.push(previous);
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

/// Holds `setup.lock` beside the setup file for as long as it lives. The
/// window and the keeper are separate processes: every change to the setup,
/// and every send of a kept setup, happens under this lock, so a device you
/// stopped keeping can't be put back by a keeper run that read the setup
/// before you did.
pub struct SetupLock(#[allow(dead_code)] std::fs::File);

impl SetupLock {
    pub fn acquire(setup_path: &Path) -> Result<SetupLock, SetupError> {
        let path = setup_path.with_extension("lock");
        let write = |source| SetupError::Write { path: path.clone(), source };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(write)?;
        }
        let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).map_err(write)?;
        file.lock().map_err(write)?;
        Ok(SetupLock(file))
    }
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
    #[error("There's no earlier setup to go back to for that device.")]
    NothingBefore,
    #[error("Its lights came on while you were deciding, so nothing was kept. Press Keep this to keep what it shows.")]
    NoLongerDark,
}

/// What happened to a device's lighting when Keeplight stopped keeping it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandBack {
    /// It was in Direct mode, which shows nothing unless a program sends
    /// colors, and now reads back in its own built-in mode, named here.
    OwnLighting(String),
    /// It was sent its own built-in mode, named first, but still reads back
    /// as the second. It may be dark until set in OpenRGB or replugged.
    NotTaken(String, String),
    /// Nothing was sent: it isn't in Direct mode, or has no built-in mode to
    /// go back to, so it keeps showing what it shows until it loses power.
    LeftAsIs,
    /// OpenRGB doesn't list it right now, so nothing was sent.
    NotListed,
}

/// The device's own lighting, for handing it back: a mode named "Onboard",
/// as the SteelSeries keyboard on this PC has.
fn own_lighting(device: &Device) -> Option<usize> {
    device.modes.iter().position(|m| m.name.eq_ignore_ascii_case("onboard"))
}

/// Finds a device by product name and location in a fresh read.
fn find_device(client: &mut Client, name: &str, location: &str) -> Result<Device, ActionError> {
    let snapshot = client.snapshot()?;
    snapshot.devices.into_iter().find(|d| d.name == name && d.location == location).ok_or(ActionError::NotFound)
}

/// Whether a keep may save a device whose lights are all off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dark {
    Refuse,
    /// The front end asked first.
    Allow,
}

/// Takes the setup lock, then [`keep_now_locked`].
pub fn keep_now(address: &str, setup_path: &Path, name: &str, location: &str, dark: Dark) -> Result<Kept, ActionError> {
    let lock = SetupLock::acquire(setup_path)?;
    keep_now_locked(&lock, address, setup_path, name, location, dark)
}

/// Keeps what the named device shows right now: reads it fresh from OpenRGB
/// at `address` and saves it into the setup at `setup_path`. Sends nothing
/// that changes a light.
pub fn keep_now_locked(
    _held: &SetupLock,
    address: &str,
    setup_path: &Path,
    name: &str,
    location: &str,
    dark: Dark,
) -> Result<Kept, ActionError> {
    // Read the setup first, so an unreadable one is never saved over.
    let mut setup = Setup::load(setup_path)?;
    let mut client = Client::connect(address)?;
    let device = find_device(&mut client, name, location)?;
    let kept = match dark {
        Dark::Refuse => Kept::capture(&device)?,
        // Asked about a dark device: keep it only if it is still dark.
        Dark::Allow if !is_dark(&device) => return Err(ActionError::NoLongerDark),
        Dark::Allow => Kept::capture_dark(&device)?,
    };
    setup.keep(kept.clone());
    setup.save(setup_path)?;
    Ok(kept)
}

/// Takes the setup lock, then [`stop_keeping_locked`].
pub fn stop_keeping(address: &str, setup_path: &Path, name: &str, location: &str) -> Result<HandBack, ActionError> {
    let lock = SetupLock::acquire(setup_path)?;
    stop_keeping_locked(&lock, address, setup_path, name, location)
}

/// Stops keeping the named device: removes it from the setup, then hands it
/// back to its own lighting when it is in Direct mode and has a built-in mode
/// to go back to. Sends that one device at most one `UPDATE_MODE`, with the
/// built-in mode exactly as OpenRGB reported it.
pub fn stop_keeping_locked(
    _held: &SetupLock,
    address: &str,
    setup_path: &Path,
    name: &str,
    location: &str,
) -> Result<HandBack, ActionError> {
    let mut setup = Setup::load(setup_path)?;
    let Some(at) = setup.devices.iter().position(|k| k.name == name && k.location == location) else {
        return Err(ActionError::NotKept);
    };
    // Remembered, so "Keep it as before" can bring it back.
    let stopped = setup.devices.remove(at);
    setup.before.retain(|k| !(k.name == name && k.location == location));
    setup.before.push(stopped);
    setup.save(setup_path)?;

    let mut client = Client::connect(address)?;
    let device = match find_device(&mut client, name, location) {
        Ok(d) => d,
        Err(ActionError::NotFound) => return Ok(HandBack::NotListed),
        Err(e) => return Err(e),
    };
    let in_direct = device.active_mode_name().is_some_and(|m| m.eq_ignore_ascii_case("direct"));
    let Some(index) = own_lighting(&device).filter(|_| in_direct) else { return Ok(HandBack::LeftAsIs) };
    let own = device.modes[index].clone();
    client.update_mode(device.index, index, &own)?;
    let after = client.controller(device.index)?;
    Ok(match after.active_mode_name() {
        Some(now) if now == own.name => HandBack::OwnLighting(own.name),
        now => HandBack::NotTaken(own.name, now.unwrap_or("no mode").to_string()),
    })
}

/// Takes the setup lock, then [`put_back_now_locked`].
pub fn put_back_now(address: &str, setup_path: &Path, name: &str, location: &str) -> Result<Outcome, ActionError> {
    let lock = SetupLock::acquire(setup_path)?;
    put_back_now_locked(&lock, address, setup_path, name, location)
}

/// Puts the named device back to what is kept for it (see [`put_back`]).
pub fn put_back_now_locked(
    _held: &SetupLock,
    address: &str,
    setup_path: &Path,
    name: &str,
    location: &str,
) -> Result<Outcome, ActionError> {
    let setup = Setup::load(setup_path)?;
    let mut client = Client::connect(address)?;
    let device = find_device(&mut client, name, location)?;
    let kept = setup.find(&device).ok_or(ActionError::NotKept)?;
    Ok(put_back(&mut client, kept, &device)?)
}

/// Takes the setup lock, then [`keep_as_before_locked`].
pub fn keep_as_before(address: &str, setup_path: &Path, name: &str, location: &str) -> Result<Outcome, ActionError> {
    let lock = SetupLock::acquire(setup_path)?;
    keep_as_before_locked(&lock, address, setup_path, name, location)
}

/// Keeps the named device as it was kept before you stopped keeping it, and
/// puts it back to that now (see [`put_back`]).
pub fn keep_as_before_locked(
    _held: &SetupLock,
    address: &str,
    setup_path: &Path,
    name: &str,
    location: &str,
) -> Result<Outcome, ActionError> {
    let mut setup = Setup::load(setup_path)?;
    let mut client = Client::connect(address)?;
    let device = find_device(&mut client, name, location)?;
    let earlier = setup.find_before(&device).cloned().ok_or(ActionError::NothingBefore)?;
    // A kept device swaps: what it was kept as becomes the earlier setup, so
    // pressing it again undoes the undo.
    let current = setup.find(&device).cloned();
    setup.keep_remembering(earlier.clone(), current);
    setup.save(setup_path)?;
    Ok(put_back(&mut client, &earlier, &device)?)
}

/// Takes the setup lock, then [`set_color_locked`].
pub fn set_color(
    address: &str,
    setup_path: &Path,
    name: &str,
    location: &str,
    color: Rgb,
) -> Result<(Kept, Outcome), ActionError> {
    let lock = SetupLock::acquire(setup_path)?;
    set_color_locked(&lock, address, setup_path, name, location, color)
}

/// Sets the named device to `color` (see [`Kept::colored`]), keeps that, and
/// sends it. What was kept before, or what it showed when it wasn't kept, is
/// remembered for "Keep it as before".
pub fn set_color_locked(
    _held: &SetupLock,
    address: &str,
    setup_path: &Path,
    name: &str,
    location: &str,
    color: Rgb,
) -> Result<(Kept, Outcome), ActionError> {
    let mut setup = Setup::load(setup_path)?;
    let mut client = Client::connect(address)?;
    let device = find_device(&mut client, name, location)?;
    let kept = Kept::colored(&device, color)?;
    let previous = undo_for(&setup, &device)?;
    setup.keep_remembering(kept.clone(), Some(previous));
    setup.save(setup_path)?;
    let outcome = put_back(&mut client, &kept, &device)?;
    Ok((kept, outcome))
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

impl Report {
    /// The device in a sentence: its product name and the end of its
    /// location, such as "ENE DRAM, address 0x73".
    pub fn label(&self) -> String {
        match self.location.rsplit(", ").next().filter(|tail| tail.len() < self.location.len()) {
            Some(tail) => format!("{}, {tail}", self.name),
            None if self.location.len() <= 24 => format!("{} at {}", self.name, self.location),
            None => self.name.clone(),
        }
    }
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
    for waited_for in &setup.devices {
        // The setup may have changed while this run waited: you may have
        // stopped keeping a device, or kept it again with other colors. Each
        // send uses the setup as it is now, under the lock.
        let _lock = SetupLock::acquire(setup_path)?;
        let now = Setup::load(setup_path)?;
        let Some(kept) = now.devices.iter().find(|k| k.name == waited_for.name && k.location == waited_for.location)
        else {
            continue;
        };
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
        let mut switched_off = testing::ram("SMBus");
        switched_off.modes.push(Mode { name: "Off".into(), ..Mode::default() });
        switched_off.active_mode = Some(3);
        assert!(is_dark(&switched_off), "the Off mode is dark even though it has no colors");
        assert_eq!(Kept::capture(&switched_off), Err(KeepError::AllOff));
        assert_eq!(Kept::capture_dark(&switched_off).unwrap().mode.name, "Off", "unless asked first");
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
        let kept = keep_now(&server.address, &path, "ENE DRAM", "SMBus 0x71", Dark::Refuse).unwrap();
        assert_eq!(kept.mode.name, "Chase Fade");
        assert_eq!(Setup::load(&path).unwrap().devices, [kept]);
        assert!(!server.seen_ids().contains(&packet::UPDATE_MODE), "keeping changes no light");

        server.state.lock().unwrap().devices[0].active_mode = Some(1);
        assert_eq!(put_back_now(&server.address, &path, "ENE DRAM", "SMBus 0x71").unwrap(), Outcome::Held);
        assert_eq!(server.device(0).active_mode_name(), Some("Chase Fade"));
        assert!(matches!(put_back_now(&server.address, &path, "Apex", "HID: keyboard"), Err(ActionError::NotKept)));
        assert!(matches!(
            keep_now(&server.address, &path, "Gone", "nowhere", Dark::Refuse),
            Err(ActionError::NotFound)
        ));
    }

    #[test]
    fn keeping_a_dark_device_needs_the_front_end_to_have_asked() {
        let path = temp_path("dark");
        let server = FakeServer::start(vec![testing::keyboard(Rgb::default())]);
        assert!(matches!(
            keep_now(&server.address, &path, "Apex", "HID: keyboard", Dark::Refuse),
            Err(ActionError::Keep(KeepError::AllOff))
        ));
        let kept = keep_now(&server.address, &path, "Apex", "HID: keyboard", Dark::Allow).unwrap();
        assert!(kept.colors.iter().all(|c| c.is_black()));
    }

    #[test]
    fn keep_it_dark_refuses_a_device_that_lit_up_meanwhile() {
        let path = temp_path("dark-lit");
        let server = FakeServer::start(vec![testing::keyboard(RED)]);
        assert!(matches!(
            keep_now(&server.address, &path, "Apex", "HID: keyboard", Dark::Allow),
            Err(ActionError::NoLongerDark)
        ));
        assert!(Setup::load(&path).unwrap().devices.is_empty(), "nothing kept");
    }

    #[test]
    fn a_device_stopped_while_a_run_waits_is_not_put_back() {
        let path = temp_path("stop-during-run");
        let mut setup = Setup::default();
        setup.keep(Kept::capture(&testing::keyboard(RED)).unwrap());
        setup.keep(Kept::capture(&testing::ram("SMBus 0x73")).unwrap());
        setup.save(&path).unwrap();
        // The keyboard is listed; the RAM stick shows up late, so the run waits.
        let mut onboard = testing::keyboard(RED);
        onboard.active_mode = Some(1);
        let server = FakeServer::start(vec![onboard]);
        let (state, address, file) = (server.state.clone(), server.address.clone(), path.clone());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            // You stop keeping the keyboard while the run waits.
            stop_keeping(&address, &file, "Apex", "HID: keyboard").unwrap();
            std::thread::sleep(Duration::from_millis(60));
            let mut rainbow = testing::ram("SMBus 0x73");
            rainbow.active_mode = Some(1);
            state.lock().unwrap().devices.push(Device { index: 1, ..rainbow });
        });
        let run = put_back_all(&server.address, &path, Duration::from_secs(3), Duration::from_millis(20)).unwrap();
        assert_eq!(run.reports.len(), 1, "only the RAM stick is still kept");
        assert_eq!(run.reports[0].location, "SMBus 0x73");
        assert_eq!(server.device(0).active_mode_name(), Some("Onboard"), "the keyboard stays handed back");
        let to_keyboard =
            server.state.lock().unwrap().seen.iter().any(|&(id, dev)| dev == 0 && id == packet::UPDATE_LEDS);
        assert!(!to_keyboard, "no kept colors were sent to the keyboard");
    }

    #[test]
    fn stopping_hands_a_direct_keyboard_back_to_its_own_lighting() {
        let path = temp_path("stop");
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71"), testing::keyboard(RED)]);
        keep_now(&server.address, &path, "ENE DRAM", "SMBus 0x71", Dark::Refuse).unwrap();
        keep_now(&server.address, &path, "Apex", "HID: keyboard", Dark::Refuse).unwrap();

        let back = stop_keeping(&server.address, &path, "Apex", "HID: keyboard").unwrap();
        assert_eq!(back, HandBack::OwnLighting("Onboard".into()));
        assert_eq!(server.device(1).active_mode_name(), Some("Onboard"));
        assert_eq!(Setup::load(&path).unwrap().devices.len(), 1, "only the keyboard left the setup");

        // RAM has no built-in mode to go back to: removed, nothing sent.
        let writes_before = server.seen_ids().iter().filter(|&&id| id == packet::UPDATE_MODE).count();
        assert_eq!(stop_keeping(&server.address, &path, "ENE DRAM", "SMBus 0x71").unwrap(), HandBack::LeftAsIs);
        let writes_after = server.seen_ids().iter().filter(|&&id| id == packet::UPDATE_MODE).count();
        assert_eq!(writes_before, writes_after);
        assert!(Setup::load(&path).unwrap().devices.is_empty());
        assert!(matches!(stop_keeping(&server.address, &path, "ENE DRAM", "SMBus 0x71"), Err(ActionError::NotKept)));
    }

    #[test]
    fn keep_it_as_before_brings_back_what_was_stopped() {
        let path = temp_path("before");
        let server = FakeServer::start(vec![testing::keyboard(RED)]);
        keep_now(&server.address, &path, "Apex", "HID: keyboard", Dark::Refuse).unwrap();
        stop_keeping(&server.address, &path, "Apex", "HID: keyboard").unwrap();
        assert_eq!(server.device(0).active_mode_name(), Some("Onboard"));
        let setup = Setup::load(&path).unwrap();
        assert!(setup.devices.is_empty() && setup.before.len() == 1, "remembered, not kept");

        assert_eq!(keep_as_before(&server.address, &path, "Apex", "HID: keyboard").unwrap(), Outcome::Held);
        assert_eq!(server.device(0).active_mode_name(), Some("Direct"));
        assert_eq!(server.device(0).colors, vec![RED; 6], "red again");
        let setup = Setup::load(&path).unwrap();
        assert_eq!(setup.devices.len(), 1);
        assert!(setup.before.is_empty(), "kept again, so nothing is remembered");
        assert!(matches!(
            keep_as_before(&server.address, &path, "Apex", "HID: keyboard"),
            Err(ActionError::NothingBefore)
        ));
    }

    #[test]
    fn a_new_color_keeps_the_effect_when_the_mode_takes_colors() {
        // Per-light effect: RAM stays Chase Fade, every light blue.
        let k = Kept::colored(&testing::ram("SMBus 0x71"), BLUE).unwrap();
        assert_eq!(k.mode.name, "Chase Fade");
        assert_eq!(k.colors, vec![BLUE; 8]);
        // A mode with its own colors: Static stays Static, its color blue.
        let k = Kept::colored(&testing::board(), BLUE).unwrap();
        assert_eq!(k.mode.name, "Static");
        assert_eq!(k.mode.colors, vec![BLUE]);
        assert!(k.colors.is_empty());
    }

    #[test]
    fn a_new_color_swaps_a_mode_without_colors_for_static_then_direct() {
        // Rainbow RAM has no Static, so Direct.
        let k = Kept::colored(&rainbow_ram("SMBus 0x71"), BLUE).unwrap();
        assert_eq!(k.mode.name, "Direct");
        assert_eq!(k.colors, vec![BLUE; 8]);
        // A keyboard on its own Onboard lighting: Direct.
        let mut kb = testing::keyboard(RED);
        kb.active_mode = Some(1);
        assert_eq!(Kept::colored(&kb, BLUE).unwrap().mode.name, "Direct");
        // A board on a rainbow: Static, ahead of Direct.
        let mut board = testing::board();
        board.modes.push(Mode { name: "Rainbow".into(), color_mode: color_mode::NONE, ..Mode::default() });
        board.active_mode = Some(2);
        assert_eq!(Kept::colored(&board, BLUE).unwrap().mode.name, "Static");
    }

    #[test]
    fn a_new_color_refuses_black_and_devices_with_no_colorable_mode() {
        assert_eq!(Kept::colored(&testing::ram("a"), Rgb::default()), Err(KeepError::Black));
        let mut fixed = testing::ram("a");
        fixed.modes.retain(|m| m.name == "Rainbow");
        fixed.active_mode = Some(0);
        assert_eq!(Kept::colored(&fixed, BLUE), Err(KeepError::NoColorMode));
    }

    #[test]
    fn setting_a_color_keeps_it_and_keep_it_as_before_swaps_back_and_forth() {
        let path = temp_path("set-color");
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        keep_now(&server.address, &path, "ENE DRAM", "SMBus 0x71", Dark::Refuse).unwrap();

        let (kept, outcome) = set_color(&server.address, &path, "ENE DRAM", "SMBus 0x71", BLUE).unwrap();
        assert_eq!(outcome, Outcome::Held);
        assert_eq!(kept.mode.name, "Chase Fade");
        assert_eq!(server.device(0).colors, vec![BLUE; 8]);
        let setup = Setup::load(&path).unwrap();
        assert_eq!(setup.devices[0].colors, vec![BLUE; 8]);
        assert_eq!(setup.before[0].colors, vec![RED; 8], "the red it had is remembered");

        // Undo: red again, and blue remembered, so pressing it again redoes.
        keep_as_before(&server.address, &path, "ENE DRAM", "SMBus 0x71").unwrap();
        assert_eq!(server.device(0).colors, vec![RED; 8]);
        let setup = Setup::load(&path).unwrap();
        assert_eq!((setup.devices.len(), setup.before.len()), (1, 1));
        assert_eq!(setup.before[0].colors, vec![BLUE; 8]);
        keep_as_before(&server.address, &path, "ENE DRAM", "SMBus 0x71").unwrap();
        assert_eq!(server.device(0).colors, vec![BLUE; 8]);
    }

    #[test]
    fn setting_a_color_on_a_device_not_kept_remembers_what_it_showed() {
        let path = temp_path("set-color-new");
        let mut kb = testing::keyboard(RED);
        kb.active_mode = Some(1);
        let server = FakeServer::start(vec![kb]);
        set_color(&server.address, &path, "Apex", "HID: keyboard", BLUE).unwrap();
        assert_eq!(server.device(0).active_mode_name(), Some("Direct"));
        assert_eq!(server.device(0).colors, vec![BLUE; 6]);
        let setup = Setup::load(&path).unwrap();
        assert_eq!(setup.before[0].mode.name, "Onboard", "Keep it as before hands it back to its own lighting");
    }

    #[test]
    fn a_new_color_is_refused_when_it_couldnt_be_undone() {
        let path = temp_path("set-color-no-undo");
        let mut unknown = testing::ram("SMBus 0x71");
        unknown.active_mode = None;
        // It can take a color (Direct), but what it shows now can't be kept.
        assert!(Kept::colored(&unknown, BLUE).is_ok());
        assert_eq!(undo_for(&Setup::default(), &unknown), Err(KeepError::NoUndo));
        let server = FakeServer::start(vec![unknown]);
        let sent_before = server.seen_ids().iter().filter(|&&id| id == packet::UPDATE_MODE).count();
        assert!(matches!(
            set_color(&server.address, &path, "ENE DRAM", "SMBus 0x71", BLUE),
            Err(ActionError::Keep(KeepError::NoUndo))
        ));
        assert_eq!(server.seen_ids().iter().filter(|&&id| id == packet::UPDATE_MODE).count(), sent_before);
        assert!(Setup::load(&path).unwrap().devices.is_empty(), "nothing saved");
    }

    #[test]
    fn keeping_anew_forgets_the_earlier_setup_and_the_keeper_never_uses_it() {
        let path = temp_path("before-forget");
        let server = FakeServer::start(vec![testing::keyboard(RED)]);
        keep_now(&server.address, &path, "Apex", "HID: keyboard", Dark::Refuse).unwrap();
        stop_keeping(&server.address, &path, "Apex", "HID: keyboard").unwrap();
        // The keeper runs: the remembered setup is not put back.
        let run = put_back_all(&server.address, &path, Duration::from_millis(200), Duration::from_millis(20)).unwrap();
        assert!(run.reports.is_empty());
        assert_eq!(server.device(0).active_mode_name(), Some("Onboard"));
        // Keeping what it shows now replaces the earlier setup.
        keep_now(&server.address, &path, "Apex", "HID: keyboard", Dark::Refuse).unwrap();
        let setup = Setup::load(&path).unwrap();
        assert_eq!(setup.devices[0].mode.name, "Onboard");
        assert!(setup.before.is_empty());
    }

    #[test]
    fn setups_written_before_the_earlier_list_still_load() {
        let path = temp_path("before-old");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{ "version": 1, "devices": [] }"#).unwrap();
        assert_eq!(Setup::load(&path).unwrap(), Setup::default());
    }

    #[test]
    fn a_keyboard_that_ignores_the_hand_back_is_reported() {
        let path = temp_path("stop-stuck");
        let server = FakeServer::start(vec![testing::keyboard(RED)]);
        keep_now(&server.address, &path, "Apex", "HID: keyboard", Dark::Refuse).unwrap();
        server.state.lock().unwrap().stuck.push(0);
        assert_eq!(
            stop_keeping(&server.address, &path, "Apex", "HID: keyboard").unwrap(),
            HandBack::NotTaken("Onboard".into(), "Direct".into())
        );
        assert!(Setup::load(&path).unwrap().devices.is_empty(), "it stops being kept either way");
    }

    #[test]
    fn stopping_a_device_openrgb_no_longer_lists_still_removes_it() {
        let path = temp_path("stop-gone");
        let mut setup = Setup::default();
        setup.keep(Kept::capture(&testing::keyboard(RED)).unwrap());
        setup.save(&path).unwrap();
        let server = FakeServer::start(vec![]);
        assert_eq!(stop_keeping(&server.address, &path, "Apex", "HID: keyboard").unwrap(), HandBack::NotListed);
        assert!(Setup::load(&path).unwrap().devices.is_empty());
    }

    #[test]
    fn keep_now_never_saves_over_an_unreadable_setup() {
        let path = temp_path("keep-corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        assert!(matches!(
            keep_now(&server.address, &path, "ENE DRAM", "SMBus 0x71", Dark::Refuse),
            Err(ActionError::Setup(_))
        ));
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
