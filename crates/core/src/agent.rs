//! Keeplight's engine as every front end calls it: the window, the command
//! line (`keeplight-cli`) and the MCP server (`keeplight-cli mcp`), following
//! the fleet's agent-interface standard (`docs/agent-interfaces.md` in
//! agentic-instructions) with the kit in `crate::agent_kit`.
//!
//! - Reads: [`Engine::devices`] and [`Engine::diagnose`].
//! - Changes: one [`Change`] per device (keep, keep dark, put back, stop
//!   keeping, keep as before). [`check`] decides what a change will do, and
//!   is the only place that does: [`Engine::plan`] saves its steps, and
//!   [`Engine::apply`] runs it again before writing and refuses if the steps
//!   differ. The window uses [`Engine::change_now`], which checks and writes
//!   at once.
//! - Every change and failure goes to the event log (`logs/keeplight-*.jsonl`);
//!   every applied plan also goes to `journal.jsonl`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent_kit::error::{AgentError, ErrorCode, Exit, Result, codes as kit};
use crate::agent_kit::events::{EventLog, LogEvent, LogLevel};
use crate::agent_kit::gate::{Authorized, Caller, authorize_write};
use crate::agent_kit::plan::{Plan, PlanStore, Precondition};
use crate::agent_kit::{diagnose, journal, now_ms, sha256_hex};
use crate::device::Rgb;
use crate::device::{Device, device_id, display_names};
use crate::keep::{self, ActionError, Dark, HandBack, KeepError, Kept, Outcome, Setup, SetupError, is_dark};
use crate::lit_by::lit_by;
use crate::paths::{Paths, running_programs_for};
use crate::sdk::{SdkError, Snapshot};
use crate::settings::Settings;

/// The version of every JSON shape below and of the error report. A breaking
/// change raises it; the shapes are committed under `schema/`.
pub const SCHEMA_VERSION: u32 = 1;

/// The one target every plan is for: this PC's lights.
pub const TARGET: &str = "keeplight";

/// Keeplight's own error codes, beside the kit's.
pub mod codes {
    use super::{ErrorCode, Exit};

    pub const DEVICE_NOT_FOUND: ErrorCode = ErrorCode::new(
        "device.not_found",
        "No device has that id or name.",
        "Read the devices (get_devices, or keeplight-cli devices) and use an id from there.",
        false,
        Exit::Refused,
    );
    pub const DEVICE_NOT_KEPT: ErrorCode = ErrorCode::new(
        "device.not_kept",
        "Keeplight doesn't keep that device, so there is nothing to put back or stop keeping.",
        "Keep it first, or choose a device whose kept state is not notKept.",
        false,
        Exit::Refused,
    );
    pub const NOTHING_BEFORE: ErrorCode = ErrorCode::new(
        "device.nothing_before",
        "That device has no earlier setup to go back to.",
        "Only a device you stopped keeping has one (keptBefore in get_devices). Keep it instead.",
        false,
        Exit::Refused,
    );
    pub const CANT_KEEP: ErrorCode = ErrorCode::new(
        "device.cant_keep",
        "That device can't be kept the way asked: every light is off (use keepDark), or its lights aren't all off \
         (use keep), or OpenRGB doesn't say which mode it's in.",
        "Read the device's dark flag and mode, then use keep or keepDark to match.",
        false,
        Exit::Refused,
    );
    pub const CANT_COLOR: ErrorCode = ErrorCode::new(
        "device.cant_color",
        "That device can't be set to that color: none of its modes takes a color, the color is black, or OpenRGB \
         doesn't say which mode it's in, so the change couldn't be undone.",
        "Pick another device, or a color other than #000000 (use keepDark to keep a device dark). For an unknown \
         mode, set the device in OpenRGB and keep it first.",
        false,
        Exit::Refused,
    );
    pub const ZONE_NOT_FOUND: ErrorCode = ErrorCode::new(
        "zone.not_found",
        "The device has no zone with that name or id.",
        "Read the device's zones (get_devices, or keeplight-cli devices --json) and use a zone id from there.",
        false,
        Exit::Refused,
    );
    pub const OPENRGB_UNREACHABLE: ErrorCode = ErrorCode::new(
        "openrgb.unreachable",
        "OpenRGB isn't answering on its SDK port, so Keeplight can't see or set any light.",
        "Start the OpenRGB service (or turn on its SDK server), then try again.",
        true,
        Exit::Failed,
    );
}

/// Every code Keeplight can return: the kit's, then its own.
pub static ALL_CODES: std::sync::LazyLock<Vec<ErrorCode>> = std::sync::LazyLock::new(|| {
    kit::with(&[
        codes::DEVICE_NOT_FOUND,
        codes::DEVICE_NOT_KEPT,
        codes::NOTHING_BEFORE,
        codes::CANT_KEEP,
        codes::CANT_COLOR,
        codes::ZONE_NOT_FOUND,
        codes::OPENRGB_UNREACHABLE,
    ])
});

/// What a change does to one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// Keep what the device shows now. Changes no light.
    Keep,
    /// Keep a device whose every light is off, so it is turned back off after
    /// sleep. Changes no light now.
    KeepDark,
    /// Send a kept device its kept setup.
    PutBack,
    /// Stop keeping it; a device in Direct mode with an Onboard mode is handed
    /// back to that, so it isn't left dark.
    StopKeeping,
    /// Keep the setup it had before the last change (stopping keeping it, or
    /// a new color), and send it that setup. On a kept device the two swap,
    /// so doing it twice undoes it.
    KeepAsBefore,
    /// Set the device to `color` and keep that. A mode that takes colors keeps
    /// its effect in the new color; otherwise it switches to Static, or Direct.
    SetColor,
}

const ACTIONS: [Action; 6] =
    [Action::Keep, Action::KeepDark, Action::PutBack, Action::StopKeeping, Action::KeepAsBefore, Action::SetColor];

impl Action {
    /// The command-line word for it.
    pub fn word(self) -> &'static str {
        match self {
            Action::Keep => "keep",
            Action::KeepDark => "keep-dark",
            Action::PutBack => "put-back",
            Action::StopKeeping => "stop-keeping",
            Action::KeepAsBefore => "keep-as-before",
            Action::SetColor => "set-color",
        }
    }

    pub fn from_word(word: &str) -> Option<Action> {
        ACTIONS.into_iter().find(|a| a.word() == word)
    }

    fn event(self) -> &'static str {
        match self {
            Action::Keep => "device.kept",
            Action::KeepDark => "device.kept_dark",
            Action::PutBack => "device.put_back",
            Action::StopKeeping => "device.stopped_keeping",
            Action::KeepAsBefore => "device.kept_as_before",
            Action::SetColor => "device.color_set",
        }
    }
}

/// One change the user asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub action: Action,
    /// The device's id ("ram-stick-1") or display name ("RAM stick 1"), as
    /// get_devices lists them.
    pub device: String,
    /// For setColor only: the color as #rrggbb, such as "#0050ff".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// For setColor only: one zone of the device, by its name or id as
    /// get_devices lists its zones ("addressable-rgb-header-1"). The other
    /// zones keep their colors. Without it, the whole device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
}

impl Change {
    pub fn new(action: Action, device: impl Into<String>) -> Change {
        Change { action, device: device.into(), color: None, zone: None }
    }

    pub fn set_color(device: impl Into<String>, color: impl Into<String>) -> Change {
        Change { action: Action::SetColor, device: device.into(), color: Some(color.into()), zone: None }
    }

    /// [`Change::set_color`] for one zone of the device.
    pub fn in_zone(self, zone: impl Into<String>) -> Change {
        Change { zone: Some(zone.into()), ..self }
    }
}

/// A plan's payload: the change and what it will do, in plain words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangePlan {
    pub change: Change,
    /// The device's display name, such as "RAM stick 1".
    pub device_name: String,
    /// The product name and location OpenRGB reported for it when planned.
    /// Applying acts on the device at that location, never on whichever one
    /// holds the same id after OpenRGB lists devices in another order.
    pub product: String,
    pub location: String,
    pub steps: Vec<String>,
}

/// What [`check`] found: the device, its name and the steps.
#[derive(Debug, Clone)]
pub struct Checked {
    pub device: Device,
    pub name: String,
    pub steps: Vec<String>,
}

/// How a device stands against what Keeplight keeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum KeptState {
    NotKept,
    /// Showing what was kept, as far as OpenRGB's record says.
    Kept,
    /// Showing something else: putting it back can fix that.
    Differs,
    /// Kept, but the device changed (light count, modes) so the setup no
    /// longer fits; it is skipped until kept again.
    Changed,
}

/// One device, as get_devices and `keeplight-cli devices` report it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceReport {
    /// Use this, or the name, in a change.
    pub id: String,
    /// What a person calls it, such as "RAM stick 1".
    pub name: String,
    /// The product name OpenRGB reports.
    pub product: String,
    pub kind: String,
    /// Where OpenRGB found it.
    pub location: String,
    pub lights: usize,
    /// The mode it's in now.
    pub mode: Option<String>,
    /// The color most of its lights show, as #rrggbb; none when every light
    /// is off or it runs a mode of its own.
    pub color: Option<String>,
    /// Every light reads as off.
    pub dark: bool,
    /// It runs a lighting mode of its own (Onboard, a built-in effect), so its
    /// colors are unknown to OpenRGB.
    pub own_lighting: bool,
    /// It can store a color itself; when false it resets on any power loss.
    pub can_store_color: bool,
    pub kept: KeptState,
    /// What Keeplight keeps for it, such as "Chase Fade in #ff0000".
    pub kept_as: Option<String>,
    /// What it was kept as before you stopped keeping it.
    pub kept_before: Option<String>,
    /// What probably lights it now, in plain words. A guess where it says
    /// "probably".
    pub lit_by: String,
    /// Its zones, in order. setColor can color one on its own.
    pub zones: Vec<ZoneReport>,
}

/// One zone of a device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZoneReport {
    /// Use this, or the name, as setColor's zone.
    pub id: String,
    pub name: String,
    pub lights: usize,
    /// The color most of its lights show, as #rrggbb; none when they are all
    /// off or the device runs a mode of its own.
    pub color: Option<String>,
}

/// The `get_devices` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DevicesReport {
    pub schema_version: u32,
    /// OpenRGB's own protocol version.
    pub openrgb_protocol: u32,
    pub devices: Vec<DeviceReport>,
    /// Whether AI tools may make changes (Settings, off by default).
    pub ai_changes: bool,
}

/// What a change did, beyond its steps. Not part of the JSON; the window
/// words it in its own notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Kept,
    PutBack(Outcome),
    StoppedKeeping(HandBack),
    /// Kept in a new color and sent it.
    ColorSet(Outcome),
}

/// The result of a change: `apply_plan`, `keeplight-cli <action> --yes`, or a
/// button in the window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub schema_version: u32,
    /// Set when it came from a plan.
    pub plan_id: Option<String>,
    pub change: Change,
    pub device_name: String,
    pub steps: Vec<String>,
    /// What happened, in a sentence.
    pub result: String,
    /// For a put back: whether OpenRGB reads the device back as kept. That is
    /// OpenRGB's record, not proof the hardware shows it.
    pub held: Option<bool>,
    /// Set when the journal couldn't be written. The change still happened.
    pub journal_problem: Option<String>,
    #[serde(skip)]
    #[schemars(skip)]
    pub effect: Option<Effect>,
}

/// The state of one device, for a plan's record precondition: its mode with
/// every setting, and its colors.
fn device_state(d: &Device) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "mode": d.active(), "colors": d.colors })).expect("device state serializes")
}

fn record_key(d: &Device) -> String {
    format!("device:{}|{}", d.name, d.location)
}

fn describe(k: &Kept) -> String {
    match k.main_color() {
        Some(c) => format!("{} in {}", k.mode.name, c.hex()),
        None => k.mode.name.clone(),
    }
}

fn from_sdk(e: SdkError) -> AgentError {
    match e {
        SdkError::Unreachable { address, .. } => AgentError::new(
            codes::OPENRGB_UNREACHABLE,
            format!("OpenRGB isn't answering at {address}. Nothing was changed."),
        ),
        other => AgentError::new(
            codes::OPENRGB_UNREACHABLE,
            format!("The connection to OpenRGB failed: {other}. Nothing more was changed."),
        ),
    }
}

fn from_setup(e: SetupError) -> AgentError {
    match e {
        SetupError::Write { .. } => AgentError::new(kit::SAVE_FAILED, e.to_string()),
        other => AgentError::new(kit::STORE_UNREADABLE, other.to_string()),
    }
}

/// An error from the keep functions, as an agent error with a stable code.
pub fn from_action(e: ActionError) -> AgentError {
    match e {
        ActionError::Sdk(e) => from_sdk(e),
        ActionError::Setup(e) => from_setup(e),
        ActionError::Keep(
            e @ (KeepError::NoColorMode
            | KeepError::Black
            | KeepError::NoUndo
            | KeepError::NoZone
            | KeepError::NoZoneColor),
        ) => AgentError::new(codes::CANT_COLOR, format!("Keeplight can't set that color: {e}")),
        ActionError::Keep(e) => AgentError::new(codes::CANT_KEEP, format!("Keeplight can't keep it: {e}")),
        ActionError::NotFound => AgentError::new(codes::DEVICE_NOT_FOUND, ActionError::NotFound.to_string()),
        ActionError::NotKept => AgentError::new(codes::DEVICE_NOT_KEPT, ActionError::NotKept.to_string()),
        ActionError::NothingBefore => AgentError::new(codes::NOTHING_BEFORE, ActionError::NothingBefore.to_string()),
        ActionError::NoLongerDark => AgentError::new(kit::CHANGED_MEANWHILE, ActionError::NoLongerDark.to_string()),
    }
}

/// A setColor change's zone, as an index into the device's zones.
fn zone_of(change: &Change, device: &Device, name: &str) -> Result<Option<usize>> {
    let Some(wanted) = change.zone.as_deref().map(str::trim).filter(|z| !z.is_empty()) else { return Ok(None) };
    match device.zone_index(wanted) {
        Some(z) => Ok(Some(z)),
        None => {
            let zones: Vec<String> = device.zones.iter().map(|z| device_id(&z.name)).collect();
            Err(AgentError::new(
                codes::ZONE_NOT_FOUND,
                format!(
                    "{name} has no zone \u{201c}{wanted}\u{201d}. Its zones: {}. Nothing was changed.",
                    zones.join(", ")
                ),
            ))
        }
    }
}

/// A setColor change's color: #rrggbb, or rrggbb.
fn parse_color(text: Option<&str>) -> Result<Rgb> {
    let usage = |what: String| AgentError::new(kit::USAGE, format!("{what} Nothing was changed."));
    let text = text
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| usage("setColor needs a color, such as #0050ff.".into()))?;
    let hex = if text.starts_with('#') { text.to_string() } else { format!("#{text}") };
    Rgb::from_hex(&hex.to_lowercase())
        .ok_or_else(|| usage(format!("\u{201c}{text}\u{201d} isn't a color: use #rrggbb, such as #0050ff.")))
}

/// Finds a device by id or display name, ignoring case, or by its exact
/// location (what the window passes, since numbering can shift when a device
/// comes or goes).
fn find<'a>(devices: &'a [Device], names: &[String], wanted: &str) -> Option<(&'a Device, String)> {
    let wanted = wanted.trim();
    devices
        .iter()
        .zip(names)
        .find(|(d, n)| d.location == wanted || device_id(n) == device_id(wanted) || n.eq_ignore_ascii_case(wanted))
        .map(|(d, n)| (d, n.clone()))
}

/// Decides what `change` will do on these devices with this setup, or why it
/// can't be done. Planning saves these steps, and applying runs this again
/// and refuses when the steps differ: it is the only place a change is
/// decided.
pub fn check(change: &Change, devices: &[Device], setup: &Setup) -> Result<Checked> {
    let names = display_names(devices);
    let Some((device, name)) = find(devices, &names, &change.device) else {
        return Err(AgentError::new(
            codes::DEVICE_NOT_FOUND,
            format!("No device is called \u{201c}{}\u{201d}. Nothing was changed.", change.device),
        ));
    };
    let kept = setup.find(device);
    let steps = match change.action {
        Action::Keep => {
            let k = Kept::capture(device).map_err(|e| {
                AgentError::new(codes::CANT_KEEP, format!("{name} can't be kept as it is: {e} Nothing was changed."))
            })?;
            vec![format!("Keep {name} as {}. No light changes now.", describe(&k))]
        }
        Action::KeepDark => {
            if !is_dark(device) {
                return Err(AgentError::new(
                    codes::CANT_KEEP,
                    format!("{name}'s lights aren't all off, so keepDark doesn't fit. Use keep. Nothing was changed."),
                ));
            }
            vec![format!(
                "Keep {name} with every light off, so it's turned back off after sleep. No light changes now."
            )]
        }
        Action::PutBack => {
            let k = kept.ok_or_else(|| {
                AgentError::new(codes::DEVICE_NOT_KEPT, format!("{name} isn't kept. Nothing was changed."))
            })?;
            vec![format!("Send {name} its kept setup: {}.", describe(k))]
        }
        Action::StopKeeping => {
            let k = kept.ok_or_else(|| {
                AgentError::new(codes::DEVICE_NOT_KEPT, format!("{name} isn't kept. Nothing was changed."))
            })?;
            let mut steps =
                vec![format!("Stop keeping {name} (kept as {}), remembering it for keepAsBefore.", describe(k))];
            let direct = device.active_mode_name().is_some_and(|m| m.eq_ignore_ascii_case("direct"));
            if direct && device.modes.iter().any(|m| m.name.eq_ignore_ascii_case("onboard")) {
                steps.push(format!("Hand {name} back to its own Onboard lighting, so it isn't left dark."));
            }
            steps
        }
        Action::KeepAsBefore => {
            let k = setup.find_before(device).ok_or_else(|| {
                AgentError::new(codes::NOTHING_BEFORE, format!("{name} has no earlier setup. Nothing was changed."))
            })?;
            vec![format!("Keep {name} as before ({}) and send it that setup.", describe(k))]
        }
        Action::SetColor => {
            let color = parse_color(change.color.as_deref())?;
            let cant = |e: KeepError| {
                AgentError::new(
                    codes::CANT_COLOR,
                    format!("{name} can't be set to {}: {e} Nothing was changed.", color.hex()),
                )
            };
            let zone = zone_of(change, device, &name)?;
            let k = match zone {
                Some(z) => Kept::colored_zone(device, kept, z, color),
                None => Kept::colored(device, color),
            }
            .map_err(cant)?;
            keep::undo_for(setup, device).map_err(cant)?;
            let was = match kept {
                Some(old) => describe(old),
                None => format!("what it shows now ({})", device.active_mode_name().unwrap_or("its current mode")),
            };
            let what = match zone {
                Some(z) => format!(
                    "{name}'s {} to {} ({}), the rest as it is,",
                    device.zones[z].name,
                    color.hex(),
                    k.mode.name
                ),
                None => format!("{name} to {}", describe(&k)),
            };
            vec![format!("Set {what} and keep it. Keep it as before goes back to {was}.")]
        }
    };
    Ok(Checked { device: device.clone(), name, steps })
}

/// Keeplight's engine for one data folder and one OpenRGB.
pub struct Engine {
    pub paths: Paths,
    pub address: String,
    events: EventLog,
}

impl Engine {
    pub fn new(paths: Paths, address: impl Into<String>) -> Engine {
        let events = EventLog::new(paths.logs(), "keeplight");
        events.prune(now_ms());
        Engine { paths, address: address.into(), events }
    }

    /// The real data folder and OpenRGB, or the ones in `KEEPLIGHT_DATA` and
    /// `KEEPLIGHT_OPENRGB`.
    pub fn from_env() -> Engine {
        Engine::new(Paths::from_env(), crate::paths::openrgb_address())
    }

    pub fn settings(&self) -> Settings {
        Settings::load(&self.paths.settings())
    }

    /// Turns "Let AI tools make changes" on or off. Only the window calls
    /// this; no tool or command can.
    pub fn set_ai_changes(&self, on: bool) -> std::io::Result<()> {
        let mut s = self.settings();
        s.ai_changes = on;
        s.save(&self.paths.settings())?;
        let words = if on { "AI tools may now make changes when asked" } else { "AI tools can no longer make changes" };
        self.log(LogLevel::Info, "settings.ai_changes", &Caller::Window, None, None, words, None);
        Ok(())
    }

    fn snapshot(&self) -> Result<Snapshot> {
        crate::sdk::snapshot(&self.address).map_err(from_sdk)
    }

    fn setup(&self) -> Result<Setup> {
        Setup::load(&self.paths.setup()).map_err(from_setup)
    }

    fn plans(&self) -> PlanStore<ChangePlan> {
        PlanStore::new(self.paths.plans(), SCHEMA_VERSION)
    }

    /// Appends one event. Best effort: a log that can't be written never
    /// stops what it describes.
    #[allow(clippy::too_many_arguments)]
    pub fn log(
        &self,
        level: LogLevel,
        event: &str,
        caller: &Caller,
        subject: Option<&str>,
        plan: Option<&str>,
        message: &str,
        error: Option<&AgentError>,
    ) {
        let _ = self.events.append(&LogEvent {
            ts_ms: now_ms(),
            level,
            event: event.into(),
            caller: caller.label(),
            target: Some(TARGET.into()),
            subject: subject.map(str::to_string),
            plan: plan.map(str::to_string),
            message: message.into(),
            error: error.map(AgentError::body),
        });
    }

    /// The newest events, newest last.
    pub fn recent_events(&self, limit: usize) -> Vec<LogEvent> {
        self.events.recent(limit)
    }

    /// Every device, how it stands against what is kept, and what probably
    /// lights it.
    pub fn devices(&self) -> Result<DevicesReport> {
        let snapshot = self.snapshot()?;
        let setup = self.setup()?;
        let running = running_programs_for(&self.address);
        let names = display_names(&snapshot.devices);
        let devices = snapshot
            .devices
            .iter()
            .zip(names)
            .map(|(d, name)| {
                let kept = setup.find(d);
                let state = match kept.map(|k| k.fit(d)) {
                    None => KeptState::NotKept,
                    Some(keep::Fit::Same) => KeptState::Kept,
                    Some(keep::Fit::Differs) => KeptState::Differs,
                    Some(keep::Fit::Incompatible(_)) => KeptState::Changed,
                };
                DeviceReport {
                    id: device_id(&name),
                    name,
                    product: d.name.clone(),
                    kind: d.kind.label().into(),
                    location: d.location.clone(),
                    lights: d.led_count(),
                    mode: d.active_mode_name().map(str::to_string),
                    color: d.lit_color().map(|c| c.hex()),
                    dark: is_dark(d),
                    own_lighting: d.own_lighting().is_some(),
                    can_store_color: d.can_store_color(),
                    kept: state,
                    kept_as: kept.map(describe),
                    kept_before: setup.find_before(d).map(describe),
                    lit_by: lit_by(d, kept.is_some(), &running).describe(),
                    zones: d
                        .zones
                        .iter()
                        .enumerate()
                        .map(|(i, z)| ZoneReport {
                            id: device_id(&z.name),
                            name: z.name.clone(),
                            lights: z.led_count as usize,
                            color: d.zone_color(i).map(|c| c.hex()),
                        })
                        .collect(),
                }
            })
            .collect();
        Ok(DevicesReport {
            schema_version: SCHEMA_VERSION,
            openrgb_protocol: snapshot.server_protocol,
            devices,
            ai_changes: self.settings().ai_changes,
        })
    }

    /// Checks a change and saves it as a plan. Changes nothing else.
    pub fn plan(&self, caller: &Caller, change: Change) -> Result<Plan<ChangePlan>> {
        let result: Result<Plan<ChangePlan>> = (|| {
            let snapshot = self.snapshot()?;
            let checked = check(&change, &snapshot.devices, &self.setup()?)?;
            let preconditions = vec![
                Precondition::file(&self.paths.setup())?,
                Precondition::record(record_key(&checked.device), Some(&device_state(&checked.device))),
            ];
            let payload = ChangePlan {
                change: change.clone(),
                device_name: checked.name,
                product: checked.device.name.clone(),
                location: checked.device.location.clone(),
                steps: checked.steps,
            };
            self.plans().save(TARGET, payload, preconditions)
        })();
        match &result {
            Ok(p) => self.log(
                LogLevel::Info,
                "plan.created",
                caller,
                Some(&p.payload.device_name),
                Some(&p.plan_id),
                &p.payload.steps.join(" "),
                None,
            ),
            Err(e) => self.log(LogLevel::Warn, "plan.refused", caller, Some(&change.device), None, &e.message, Some(e)),
        }
        result
    }

    /// Applies a saved plan once, if nothing it depends on changed and the
    /// check gives the same steps. AI tools need "Let AI tools make changes"
    /// on and the user's request in their own words.
    pub fn apply(&self, caller: &Caller, plan_id: &str, request: Option<&str>) -> Result<Applied> {
        let result: Result<Applied> = (|| {
            let write = authorize_write(caller, self.settings().ai_changes, request)?;
            // Held from the check through the write, so nothing changes the
            // setup in between (the window, the keeper, another command line).
            let lock = keep::SetupLock::acquire(&self.paths.setup()).map_err(from_setup)?;
            let snapshot = self.snapshot()?;
            let setup = self.setup()?;
            let record = |key: &str| -> Result<Option<String>> {
                Ok(snapshot.devices.iter().find(|d| record_key(d) == key).map(|d| sha256_hex(&device_state(d))))
            };
            // The planned device by its location, not by its id: ids are
            // positions, and OpenRGB can list devices in another order.
            let recheck = |p: &ChangePlan| {
                let pinned = Change { device: p.location.clone(), ..p.change.clone() };
                let again = check(&pinned, &snapshot.devices, &setup)?;
                // The same physical device may now be numbered differently
                // ("RAM stick 2" instead of "RAM stick 1"); compare its steps
                // under the name it was planned with.
                let steps: Vec<String> = again.steps.iter().map(|s| s.replace(&again.name, &p.device_name)).collect();
                if again.device.name != p.product || steps != p.steps {
                    return Err(AgentError::new(
                        kit::CHANGED_MEANWHILE,
                        "The change would now do something else than planned. Plan it again. Nothing was changed.",
                    ));
                }
                Ok(again)
            };
            let plan: Plan<ChangePlan> =
                self.plans().take(&write, plan_id, TARGET, record, |p| recheck(&p.payload).map(|_| ()))?;
            let checked = recheck(&plan.payload)?;
            let mut applied = self.perform(&lock, &write, &plan.payload.change, &checked)?;
            drop(lock);
            applied.plan_id = Some(plan.plan_id.clone());
            applied.journal_problem =
                journal::append(&self.paths.data, &write, TARGET, &plan.plan_id, &checked.steps, None)
                    .err()
                    .map(|e| format!("The change was made, but the journal couldn't be written: {e}"));
            Ok(applied)
        })();
        if let Err(e) = &result {
            self.log(LogLevel::Warn, "plan.refused", caller, None, Some(plan_id), &e.message, Some(e));
        }
        result
    }

    /// Checks and makes a change at once: the window's buttons. The command
    /// line plans and applies instead, so its changes are journaled.
    pub fn change_now(&self, caller: &Caller, change: Change) -> Result<Applied> {
        let result: Result<Applied> = (|| {
            let write = authorize_write(caller, self.settings().ai_changes, None)?;
            let lock = keep::SetupLock::acquire(&self.paths.setup()).map_err(from_setup)?;
            let snapshot = self.snapshot()?;
            let checked = check(&change, &snapshot.devices, &self.setup()?)?;
            self.perform(&lock, &write, &change, &checked)
        })();
        if let Err(e) = &result {
            self.log(LogLevel::Warn, "change.refused", caller, Some(&change.device), None, &e.message, Some(e));
        }
        result
    }

    /// The one write path, after a check, under the setup lock the check was
    /// made under, for an authorized caller.
    fn perform(
        &self,
        lock: &keep::SetupLock,
        write: &Authorized,
        change: &Change,
        checked: &Checked,
    ) -> Result<Applied> {
        let (address, setup, d, name) = (&self.address, self.paths.setup(), &checked.device, checked.name.as_str());
        let (effect, result, held) = match change.action {
            Action::Keep | Action::KeepDark => {
                let dark = if change.action == Action::KeepDark { Dark::Allow } else { Dark::Refuse };
                let k =
                    keep::keep_now_locked(lock, address, &setup, &d.name, &d.location, dark).map_err(from_action)?;
                (Effect::Kept, format!("Keeping {name} as {}.", describe(&k)), None)
            }
            Action::PutBack | Action::KeepAsBefore => {
                let outcome = if change.action == Action::PutBack {
                    keep::put_back_now_locked(lock, address, &setup, &d.name, &d.location)
                } else {
                    keep::keep_as_before_locked(lock, address, &setup, &d.name, &d.location)
                }
                .map_err(from_action)?;
                let (words, held) = match &outcome {
                    Outcome::Held => (format!("Put {name} back."), Some(true)),
                    Outcome::NotHeld => {
                        (format!("Sent {name} its kept setup, but it reads back as something else."), Some(false))
                    }
                    Outcome::Skipped(reason) => (format!("Didn't send {name} anything: {reason}."), None),
                };
                (Effect::PutBack(outcome), words, held)
            }
            Action::StopKeeping => {
                let back =
                    keep::stop_keeping_locked(lock, address, &setup, &d.name, &d.location).map_err(from_action)?;
                let words = match &back {
                    HandBack::OwnLighting(m) => format!("Stopped keeping {name}; it's back on its own {m} lighting."),
                    HandBack::NotTaken(m, now) => {
                        format!("Stopped keeping {name}; it was sent its own {m} lighting but still reports {now}.")
                    }
                    HandBack::LeftAsIs => format!("Stopped keeping {name}; it keeps showing what it shows now."),
                    HandBack::NotListed => format!("Stopped keeping {name}; OpenRGB doesn't list it right now."),
                };
                (Effect::StoppedKeeping(back), words, None)
            }
            Action::SetColor => {
                let color = parse_color(change.color.as_deref())?;
                let zone = zone_of(change, d, name)?;
                let (k, outcome) = keep::set_color_locked(lock, address, &setup, &d.name, &d.location, zone, color)
                    .map_err(from_action)?;
                let what = match zone {
                    Some(z) => format!("{name}'s {} in {}", d.zones[z].name, color.hex()),
                    None => format!("{name} as {}", describe(&k)),
                };
                let (words, held) = match &outcome {
                    Outcome::Held => (format!("Set and kept {what}."), Some(true)),
                    Outcome::NotHeld => (
                        format!("Keeping {what}, but it reads back as something else after being sent it."),
                        Some(false),
                    ),
                    Outcome::Skipped(reason) => (format!("Keeping {what}, but didn't send it: {reason}."), None),
                };
                (Effect::ColorSet(outcome), words, held)
            }
        };
        let level = if held == Some(false) { LogLevel::Warn } else { LogLevel::Info };
        self.log(level, change.action.event(), write.caller(), Some(name), None, &result, None);
        Ok(Applied {
            schema_version: SCHEMA_VERSION,
            plan_id: None,
            change: change.clone(),
            device_name: name.to_string(),
            steps: checked.steps.clone(),
            result,
            held,
            journal_problem: None,
            effect: Some(effect),
        })
    }

    /// Everything for working out a problem, redacted: OpenRGB, the devices,
    /// the setup in counts, the settings, the keeper's last runs, `extra`
    /// sections from the front end, and the newest events.
    pub fn diagnose(&self, extra: serde_json::Value) -> serde_json::Value {
        let devices = match self.devices() {
            Ok(r) => serde_json::to_value(&r).expect("a report serializes"),
            Err(e) => serde_json::json!({ "error": e.body() }),
        };
        let setup = match Setup::load(&self.paths.setup()) {
            Ok(s) => serde_json::json!({ "readable": true, "kept": s.devices.len(), "keptBefore": s.before.len() }),
            Err(e) => serde_json::json!({ "readable": false, "problem": e.to_string() }),
        };
        let keeper_runs: Vec<String> = std::fs::read_to_string(self.paths.keeper_log())
            .map(|t| t.lines().rev().take(10).map(str::to_string).collect::<Vec<_>>().into_iter().rev().collect())
            .unwrap_or_default();
        let settings = self.settings();
        let sections = serde_json::json!({
            "openrgb": { "address": self.address, "thisPc": crate::paths::is_this_pc(&self.address) },
            "devices": devices,
            "setup": setup,
            "settings": { "aiChanges": settings.ai_changes, "trayIcon": settings.tray_icon },
            "keeperRuns": keeper_runs,
            "app": extra,
        });
        diagnose::bundle(
            "keeplight",
            env!("CARGO_PKG_VERSION"),
            SCHEMA_VERSION,
            sections,
            &self.recent_events(diagnose::RECENT_EVENTS),
            &diagnose::Redactor::from_env(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::agent_kit::error::check_codes;
    use crate::testing::{self, FakeServer, RED};

    pub(crate) struct Data(pub PathBuf);
    impl Drop for Data {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub(crate) fn data(name: &str) -> Data {
        let dir = std::env::temp_dir().join(format!("keeplight-agent-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Data(dir)
    }

    fn engine(server: &FakeServer, d: &Data) -> Engine {
        Engine::new(Paths { data: d.0.clone() }, server.address.clone())
    }

    fn change(action: Action, device: &str) -> Change {
        Change::new(action, device)
    }

    #[test]
    fn every_code_is_listed_once_with_a_meaning_and_a_hint() {
        check_codes(&ALL_CODES).unwrap();
    }

    #[test]
    fn devices_report_how_each_stands() {
        let d = data("devices");
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71"), testing::keyboard(RED)]);
        let e = engine(&server, &d);
        let r = e.devices().unwrap();
        assert_eq!(r.devices.len(), 2);
        assert_eq!(r.devices[0].id, "ram-stick");
        assert_eq!(r.devices[0].kept, KeptState::NotKept);
        assert!(!r.ai_changes);
        e.change_now(&Caller::Window, change(Action::Keep, "RAM stick")).unwrap();
        let r = e.devices().unwrap();
        assert_eq!(r.devices[0].kept, KeptState::Kept);
        assert_eq!(r.devices[0].kept_as.as_deref(), Some("Chase Fade in #ff0000"));
        assert_eq!(r.devices[0].lit_by, "Keeplight, which keeps it");
    }

    #[test]
    fn a_plan_is_applied_once_and_journaled() {
        let d = data("plan");
        let mut rainbow = testing::ram("SMBus 0x71");
        rainbow.active_mode = Some(1);
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        let e = engine(&server, &d);
        e.change_now(&Caller::Window, change(Action::Keep, "ram-stick")).unwrap();
        server.state.lock().unwrap().devices[0].active_mode = Some(1);

        let plan = e.plan(&Caller::Cli, change(Action::PutBack, "ram-stick")).unwrap();
        assert_eq!(plan.payload.steps, ["Send RAM stick its kept setup: Chase Fade in #ff0000."]);
        let applied = e.apply(&Caller::Cli, &plan.plan_id, None).unwrap();
        assert_eq!(applied.held, Some(true));
        assert_eq!(server.device(0).active_mode_name(), Some("Chase Fade"));
        assert_eq!(e.apply(&Caller::Cli, &plan.plan_id, None).unwrap_err().code, kit::PLAN_NOT_FOUND);
        let entries = journal::read(&d.0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].caller, "cli");
        assert!(e.recent_events(50).iter().any(|ev| ev.event == "device.put_back"));
    }

    #[test]
    fn ai_tools_need_the_switch_and_a_request() {
        let d = data("ai");
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        let e = engine(&server, &d);
        let ai = Caller::Mcp { client: "test".into() };
        let plan = e.plan(&ai, change(Action::Keep, "RAM stick")).unwrap();
        assert_eq!(e.apply(&ai, &plan.plan_id, Some("keep it")).unwrap_err().code, kit::AI_CHANGES_OFF);
        assert!(Setup::load(&e.paths.setup()).unwrap().devices.is_empty(), "nothing kept");
        e.set_ai_changes(true).unwrap();
        assert_eq!(e.apply(&ai, &plan.plan_id, Some(" ")).unwrap_err().code, kit::AI_REQUEST_MISSING);
        e.apply(&ai, &plan.plan_id, Some("keep the RAM")).unwrap();
        assert_eq!(journal::read(&d.0)[0].request.as_deref(), Some("keep the RAM"));
        assert_eq!(e.change_now(&ai, change(Action::Keep, "RAM stick")).unwrap_err().code, kit::AI_REQUEST_MISSING);
    }

    #[test]
    fn a_device_that_changed_since_the_plan_is_refused() {
        let d = data("changed");
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        let e = engine(&server, &d);
        let plan = e.plan(&Caller::Cli, change(Action::Keep, "RAM stick")).unwrap();
        server.state.lock().unwrap().devices[0].colors[0] = crate::device::Rgb { r: 0, g: 0, b: 255 };
        assert_eq!(e.apply(&Caller::Cli, &plan.plan_id, None).unwrap_err().code, kit::CHANGED_MEANWHILE);
        assert!(Setup::load(&e.paths.setup()).unwrap().devices.is_empty());
    }

    #[test]
    fn a_plan_follows_its_device_when_openrgb_reorders_them() {
        let d = data("reorder");
        // Two identical sticks: the plan for ram-stick-1 is for the one at 0x71.
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71"), testing::ram("SMBus 0x73")]);
        let e = engine(&server, &d);
        let plan = e.plan(&Caller::Cli, change(Action::Keep, "ram-stick-1")).unwrap();
        assert_eq!(plan.payload.location, "SMBus 0x71");
        // OpenRGB now lists them the other way round.
        server.state.lock().unwrap().devices.reverse();
        e.apply(&Caller::Cli, &plan.plan_id, None).unwrap();
        let setup = Setup::load(&e.paths.setup()).unwrap();
        assert_eq!(setup.devices.len(), 1);
        assert_eq!(setup.devices[0].location, "SMBus 0x71", "the planned stick, not the one now first");
    }

    #[test]
    fn the_setup_lock_is_held_from_the_check_through_the_write() {
        let d = data("held");
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        let e = engine(&server, &d);
        let plan = e.plan(&Caller::Cli, change(Action::Keep, "ram-stick")).unwrap();
        // While another process holds the lock, apply waits rather than
        // checking and writing around it.
        let held = keep::SetupLock::acquire(&e.paths.setup()).unwrap();
        let (paths, address, id) = (e.paths.clone(), e.address.clone(), plan.plan_id.clone());
        let waiter = std::thread::spawn(move || Engine::new(paths, address).apply(&Caller::Cli, &id, None).map(|_| ()));
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!waiter.is_finished(), "apply waits for the lock");
        drop(held);
        waiter.join().unwrap().unwrap();
    }

    #[test]
    fn set_color_is_planned_applied_and_undone() {
        let d = data("set-color");
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        let e = engine(&server, &d);
        let plan = e.plan(&Caller::Cli, Change::set_color("ram-stick", "0050FF")).unwrap();
        assert_eq!(
            plan.payload.steps,
            [
                "Set RAM stick to Chase Fade in #0050ff and keep it. Keep it as before goes back to what it shows now (Chase Fade)."
            ]
        );
        let applied = e.apply(&Caller::Cli, &plan.plan_id, None).unwrap();
        assert_eq!(applied.held, Some(true));
        assert_eq!(server.device(0).colors[0], crate::device::Rgb { r: 0, g: 0x50, b: 0xff });
        let r = e.devices().unwrap();
        assert_eq!(r.devices[0].kept_as.as_deref(), Some("Chase Fade in #0050ff"));
        assert_eq!(r.devices[0].kept_before.as_deref(), Some("Chase Fade in #ff0000"));
        e.change_now(&Caller::Window, change(Action::KeepAsBefore, "ram-stick")).unwrap();
        assert_eq!(server.device(0).colors[0], RED);
        assert!(e.recent_events(50).iter().any(|ev| ev.event == "device.color_set"));
    }

    #[test]
    fn set_color_on_one_zone_is_planned_by_zone_id() {
        let d = data("zone");
        let mut board = testing::board();
        board.zones = vec![
            crate::device::Zone { name: "Board".into(), led_count: 2, matrix: None },
            crate::device::Zone { name: "Addressable Header 1".into(), led_count: 2, matrix: None },
        ];
        let server = FakeServer::start(vec![board]);
        let e = engine(&server, &d);
        let r = e.devices().unwrap();
        assert_eq!(r.devices[0].zones[1].id, "addressable-header-1");
        assert_eq!(r.devices[0].zones[1].color.as_deref(), Some("#ff0000"));
        let change = Change::set_color("motherboard", "#0050ff").in_zone("addressable-header-1");
        let plan = e.plan(&Caller::Cli, change).unwrap();
        assert_eq!(
            plan.payload.steps,
            ["Set Motherboard's Addressable Header 1 to #0050ff (Direct), the rest as it is, and keep it. Keep it as \
              before goes back to what it shows now (Static)."]
        );
        let applied = e.apply(&Caller::Cli, &plan.plan_id, None).unwrap();
        assert_eq!(applied.result, "Set and kept Motherboard's Addressable Header 1 in #0050ff.");
        let blue = crate::device::Rgb { r: 0, g: 0x50, b: 0xff };
        assert_eq!(server.device(0).colors, vec![RED, RED, blue, blue]);
        let missing = Change::set_color("motherboard", "#0050ff").in_zone("nope");
        assert_eq!(e.plan(&Caller::Cli, missing).unwrap_err().code, codes::ZONE_NOT_FOUND);
    }

    #[test]
    fn set_color_needs_a_real_color() {
        let devices = [testing::ram("a")];
        let setup = Setup::default();
        let code = |c: Option<&str>| {
            let change = Change { color: c.map(str::to_string), ..Change::new(Action::SetColor, "ram-stick") };
            check(&change, &devices, &setup).unwrap_err().code
        };
        assert_eq!(code(None), kit::USAGE);
        assert_eq!(code(Some("blue")), kit::USAGE);
        assert_eq!(code(Some("#000000")), codes::CANT_COLOR);
        let mut unknown = testing::ram("a");
        unknown.active_mode = None;
        let change = Change::set_color("ram-stick", "#0050ff");
        let e = check(&change, &[unknown], &setup).unwrap_err();
        assert_eq!(e.code, codes::CANT_COLOR, "no undo, so no plan promises one");
    }

    #[test]
    fn checks_name_what_is_wrong() {
        let devices = [testing::ram("a"), testing::keyboard(crate::device::Rgb::default())];
        let setup = Setup::default();
        let code = |a, dev: &str| check(&change(a, dev), &devices, &setup).unwrap_err().code;
        assert_eq!(code(Action::Keep, "nope"), codes::DEVICE_NOT_FOUND);
        assert_eq!(code(Action::PutBack, "ram-stick"), codes::DEVICE_NOT_KEPT);
        assert_eq!(code(Action::StopKeeping, "ram-stick"), codes::DEVICE_NOT_KEPT);
        assert_eq!(code(Action::KeepAsBefore, "ram-stick"), codes::NOTHING_BEFORE);
        assert_eq!(code(Action::Keep, "keyboard"), codes::CANT_KEEP, "dark: keep refuses");
        assert_eq!(code(Action::KeepDark, "ram-stick"), codes::CANT_KEEP, "lit: keepDark refuses");
        assert!(check(&change(Action::KeepDark, "Keyboard"), &devices, &setup).is_ok());
        assert_eq!(check(&change(Action::KeepDark, "HID: keyboard"), &devices, &setup).unwrap().name, "Keyboard");
    }

    #[test]
    fn an_unreachable_openrgb_has_its_own_code() {
        let d = data("unreachable");
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
        let e = Engine::new(Paths { data: d.0.clone() }, closed);
        let err = e.devices().unwrap_err();
        assert_eq!(err.code, codes::OPENRGB_UNREACHABLE);
        assert!(err.code.retryable);
    }

    #[test]
    fn diagnose_holds_counts_and_redacts_the_home_folder() {
        let d = data("diagnose");
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        let e = engine(&server, &d);
        e.change_now(&Caller::Window, change(Action::Keep, "RAM stick")).unwrap();
        let v = e.diagnose(serde_json::json!({ "signInSwitch": true }));
        assert_eq!(v["sections"]["setup"]["kept"], 1);
        assert_eq!(v["sections"]["app"]["signInSwitch"], true);
        assert_eq!(v["app"], "keeplight");
        if let Ok(home) = std::env::var("USERPROFILE") {
            assert!(!v.to_string().to_lowercase().contains(&home.to_lowercase().replace('\\', "\\\\")));
        }
    }

    /// The JSON shapes are committed under `schema/` at the repository root
    /// (agent-interface standard, part L). Regenerate them with
    /// `KEEPLIGHT_WRITE_SCHEMAS=1 cargo test -p keeplight-core schemas`.
    #[test]
    fn schemas_are_committed() {
        use crate::agent_kit::error::{ErrorCodeList, ErrorReport};
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema");
        let shapes = [
            ("devices.schema.json", schemars::schema_for!(DevicesReport)),
            ("change.schema.json", schemars::schema_for!(Change)),
            ("plan.schema.json", schemars::schema_for!(Plan<ChangePlan>)),
            ("applied.schema.json", schemars::schema_for!(Applied)),
            ("error-report.schema.json", schemars::schema_for!(ErrorReport)),
            ("error-codes.schema.json", schemars::schema_for!(ErrorCodeList)),
        ];
        let write = std::env::var_os("KEEPLIGHT_WRITE_SCHEMAS").is_some();
        for (name, schema) in shapes {
            let text = serde_json::to_string_pretty(&schema).unwrap() + "\n";
            let path = dir.join(name);
            if write {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &text).unwrap();
            } else {
                let committed = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
                assert_eq!(committed, text, "schema/{name} is stale: run with KEEPLIGHT_WRITE_SCHEMAS=1");
            }
        }
    }

    #[test]
    fn actions_have_command_words() {
        for a in ACTIONS {
            assert_eq!(Action::from_word(a.word()), Some(a));
        }
        assert_eq!(Action::from_word("nope"), None);
    }
}
