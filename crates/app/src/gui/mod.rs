//! The main window. It lists every light OpenRGB reports, refreshing every
//! few seconds. Keep saves what a device shows into the setup; Put back sends
//! a device its kept setup. Both run in `keeplight_core::keep`; this module
//! only passes requests to it and shows the results.

mod view;

use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use keeplight_core::agent::{Action, Change, Effect, Engine};
use keeplight_core::agent_kit::gate::Caller;
use keeplight_core::keep::{HandBack, Outcome, Setup};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{Color, ComponentHandle, Model, ModelRc, VecModel};

use crate::{AppWindow, DeviceRow, Ui, ZoneItem};

const REFRESH: Duration = Duration::from_secs(3);

pub fn run() -> i32 {
    let ui = match AppWindow::new() {
        Ok(ui) => ui,
        Err(e) => {
            crate::show_message("Keeplight could not start", &e.to_string());
            return 1;
        }
    };
    let rows = Rc::new(VecModel::<DeviceRow>::default());
    let g = ui.global::<Ui>();
    g.set_version(env!("CARGO_PKG_VERSION").into());
    g.set_devices(ModelRc::from(rows.clone()));

    // OpenRGB can take seconds to answer while it scans, so it is read, and
    // every action runs, on a worker thread; only the result comes back to the
    // window.
    let (tx, rx) = mpsc::channel::<Command>();
    let address = crate::platform::openrgb_address();
    let setup_path = crate::platform::setup_path();
    let engine = Engine::new(keeplight_core::paths::Paths::from_env(), address.clone());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let mut notice: Option<view::Notice> = None;
        loop {
            let result = keeplight_core::sdk::snapshot(&address).map_err(|e| e.to_string());
            let setup = Setup::load(&setup_path).map_err(|e| e.to_string());
            // This PC's programs only say something about devices on this PC.
            let running = keeplight_core::paths::running_programs_for(&address);
            let mut v = view::build(&result, &setup, &address, notice.as_ref(), &running);
            let kept = setup.as_ref().map_or(0, |s| s.devices.len());
            let this_exe = crate::platform::startup::command_for_this_exe().ok();
            (v.sign_in, v.sign_in_note) =
                view::sign_in(crate::platform::startup::current().as_deref(), this_exe.as_deref(), kept);
            v.last_run = crate::platform::last_keeper_run().unwrap_or_default();
            let settings = keeplight_core::settings::Settings::load(&crate::platform::settings_path());
            v.tray_icon = settings.tray_icon;
            v.ai_changes = settings.ai_changes;
            if weak.upgrade_in_event_loop(move |ui| apply(&ui, v)).is_err() {
                return;
            }
            match rx.recv_timeout(REFRESH) {
                Ok(command) => notice = Some(run_command(&command, &engine)),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    let send = |make: fn(Target) -> Command| {
        let tx = tx.clone();
        let weak = ui.as_weak();
        move |row: DeviceRow| {
            if let Some(ui) = weak.upgrade() {
                ui.global::<Ui>().set_busy(true);
            }
            let _ = tx.send(make(Target::of(&row)));
        }
    };
    g.on_keep(send(Command::Keep));
    g.on_put_back(send(Command::PutBack));
    g.on_keep_dark(send(Command::KeepDark));
    g.on_stop_keeping(send(Command::StopKeeping));
    g.on_keep_as_before(send(Command::KeepAsBefore));
    {
        let tx = tx.clone();
        let weak = ui.as_weak();
        let tx2 = tx.clone();
        let weak2 = ui.as_weak();
        g.on_set_everything(move |color| {
            if let Some(ui) = weak2.upgrade() {
                ui.global::<Ui>().set_busy(true);
            }
            let _ = tx2.send(Command::SetEverything(color.to_string()));
        });
        g.on_set_color(move |row, color, zone| {
            if let Some(ui) = weak.upgrade() {
                ui.global::<Ui>().set_busy(true);
            }
            let _ = tx.send(Command::SetColor(Target::of(&row), color.to_string(), zone.to_string()));
        });
    }
    {
        let tx = tx.clone();
        g.on_set_sign_in(move |on| {
            let _ = tx.send(Command::SignIn(on));
        });
    }
    {
        let tx = tx.clone();
        g.on_set_tray_icon(move |on| {
            let _ = tx.send(Command::TrayIcon(on));
        });
    }
    {
        let tx = tx.clone();
        g.on_set_ai_changes(move |on| {
            let _ = tx.send(Command::AiChanges(on));
        });
    }
    // The lines that connect an AI app to Keeplight's MCP server, with the
    // real path to keeplight-cli.exe beside this exe.
    let [claude, gemini, codex] = connect_commands(&cli_path());
    g.set_mcp_claude(claude.into());
    g.set_mcp_gemini(gemini.into());
    g.set_mcp_codex(codex.into());

    let _repaint = repaint_after_restore(&ui);
    match ui.run() {
        Ok(()) => 0,
        Err(e) => {
            crate::show_message("Keeplight stopped", &e.to_string());
            1
        }
    }
}

/// The device a button press is for: its display name, for the notice, and
/// its location, which finds it again. Rows hold Slint models, which stay on
/// the window's thread, so only these go to the worker.
struct Target {
    name: String,
    location: String,
}

impl Target {
    fn of(row: &DeviceRow) -> Target {
        Target { name: row.name.to_string(), location: row.location_key.to_string() }
    }
}

/// A button press, carrying the device it is for.
enum Command {
    Keep(Target),
    /// Keep a device whose lights are all off, after the row asked.
    KeepDark(Target),
    PutBack(Target),
    StopKeeping(Target),
    /// Keep a device again as it was before you stopped keeping it.
    KeepAsBefore(Target),
    /// Set a device, or one zone of it (by name; empty for the whole device),
    /// to a color (#rrggbb) and keep it.
    SetColor(Target, String, String),
    /// One color for every device that can take it.
    SetEverything(String),
    /// Turn the keeper at sign-in on or off.
    SignIn(bool),
    /// Show or hide the keeper's icon in the notification area.
    TrayIcon(bool),
    /// "Let AI tools make changes".
    AiChanges(bool),
}

/// A device button: the engine checks and makes the change, logs it as
/// `gui`, and says what happened; this words it for the notice line.
fn device_change(engine: &Engine, action: Action, row: &Target) -> view::Notice {
    color_change(engine, Change::new(action, row.location.clone()), row)
}

/// [device_change] for any change, including a new color.
fn color_change(engine: &Engine, change: Change, row: &Target) -> view::Notice {
    let note = |text: String, warn: bool| view::Notice { text, warn };
    let name = &row.name;
    let action = change.action;
    let applied = match engine.change_now(&Caller::Window, change) {
        Ok(a) => a,
        Err(e) => {
            let what = match action {
                Action::Keep | Action::KeepDark => "keep",
                Action::PutBack => "put back",
                Action::StopKeeping => "stop keeping",
                Action::KeepAsBefore => "keep as before",
                Action::SetColor => "set the color of",
            };
            return note(format!("Couldn't {what} {name}. {}", e.message), true);
        }
    };
    match (action, applied.effect) {
        (Action::KeepDark, _) => note(format!("Kept {name} dark. The keeper turns it back off after sleep."), false),
        (_, Some(Effect::Kept)) => note(format!("Kept {name}. Put back returns it to what it shows now."), false),
        (Action::KeepAsBefore, Some(Effect::PutBack(o))) => match o {
            Outcome::Held => note(format!("Keeping {name} as before, and put it back."), false),
            Outcome::NotHeld => note(
                format!("Keeping {name} as before, but it reads back as something else after being sent it."),
                true,
            ),
            Outcome::Skipped(reason) => note(format!("Keeping {name} as before, but didn't send it: {reason}."), true),
        },
        (_, Some(Effect::ColorSet(o))) => match o {
            Outcome::Held => {
                note(format!("{}. The keeper keeps it that way.", applied.result.trim_end_matches('.')), false)
            }
            Outcome::NotHeld => note(format!("{} Another program may be setting it.", applied.result), true),
            Outcome::Skipped(_) => note(applied.result, true),
        },
        (_, Some(Effect::PutBack(o))) => match o {
            Outcome::Held => note(format!("Put {name} back."), false),
            Outcome::NotHeld => note(
                format!(
                    "Sent {name} its kept setup, but it reads back as something else. Another program may be setting \
                     it, or it can't show that setup."
                ),
                true,
            ),
            Outcome::Skipped(reason) => note(format!("Didn't change {name}: {reason}."), true),
        },
        (_, Some(Effect::StoppedKeeping(back))) => match back {
            HandBack::OwnLighting(mode) => {
                note(format!("Stopped keeping {name}. It's back on its own {mode} lighting."), false)
            }
            HandBack::NotTaken(mode, now) => note(
                format!(
                    "Stopped keeping {name}. It was sent its own {mode} lighting but still reports {now}. If it's dark, \
                     set it in OpenRGB or unplug it and plug it back in."
                ),
                true,
            ),
            HandBack::LeftAsIs => {
                note(format!("Stopped keeping {name}. It keeps showing what it shows now until it loses power."), false)
            }
            HandBack::NotListed => {
                note(format!("Stopped keeping {name}. OpenRGB doesn't list it right now, so nothing was sent."), false)
            }
        },
        (_, None) => note(applied.result, false),
    }
}

fn run_command(command: &Command, engine: &Engine) -> view::Notice {
    let note = |text: String, warn: bool| view::Notice { text, warn };
    match command {
        Command::Keep(row) => device_change(engine, Action::Keep, row),
        Command::KeepDark(row) => device_change(engine, Action::KeepDark, row),
        Command::PutBack(row) => device_change(engine, Action::PutBack, row),
        Command::StopKeeping(row) => device_change(engine, Action::StopKeeping, row),
        Command::KeepAsBefore(row) => device_change(engine, Action::KeepAsBefore, row),
        Command::SetEverything(color) => match engine.set_everything(&Caller::Window, color) {
            Ok(r) => {
                let mut text = format!("Set {} of {} devices to {} and kept them.", r.set(), r.devices.len(), r.color);
                for d in r.devices.iter().filter(|d| d.problem.is_some()) {
                    text.push_str(&format!(" {}: {}", d.name, d.problem.as_deref().unwrap_or_default()));
                }
                note(text, r.set() < r.devices.len())
            }
            Err(e) => note(format!("Couldn't set everything. {}", e.message), true),
        },
        Command::SetColor(row, color, zone) => {
            let change = Change::set_color(row.location.clone(), color.clone());
            let change = if zone.is_empty() { change } else { change.in_zone(zone.clone()) };
            color_change(engine, change, row)
        }
        Command::AiChanges(on) => match (engine.set_ai_changes(*on), on) {
            (Ok(()), true) => note(
                "AI tools you connect can now change what Keeplight keeps and put lights back, when you ask them to."
                    .into(),
                false,
            ),
            (Ok(()), false) => note("AI tools can read and plan, but change nothing.".into(), false),
            (Err(e), _) => note(format!("Couldn't save that setting. {e}"), true),
        },
        Command::SignIn(on) => {
            use crate::platform::startup;
            let result = if *on {
                // Start the keeper now too, rather than at the next sign-in.
                startup::command_for_this_exe()
                    .and_then(|c| startup::set(Some(&c)))
                    .and_then(|()| crate::platform::start_keeper())
            } else {
                // Entry first, then the signal: a keeper still starting up
                // sees the entry gone (see keeper::run).
                let removed = startup::set(None);
                crate::platform::keeper_stop::request();
                removed
            };
            match (result, on) {
                (Ok(()), true) => note(
                    "The keeper is running. It puts your kept devices back at each sign-in, after sleep and when \
                     OpenRGB's devices change."
                        .into(),
                    false,
                ),
                (Ok(()), false) => note("The keeper has stopped and won't run at sign-in.".into(), false),
                (Err(e), _) => note(format!("Couldn't change the sign-in setting. {e}"), true),
            }
        }
        Command::TrayIcon(on) => {
            let path = crate::platform::settings_path();
            let mut settings = keeplight_core::settings::Settings::load(&path);
            settings.tray_icon = *on;
            match (settings.save(&path), on) {
                (Ok(()), true) => note("The keeper's icon shows in the notification area while it runs.".into(), false),
                (Ok(()), false) => note(
                    "The keeper's icon is hidden. It still appears for a moment if a light doesn't take its color."
                        .into(),
                    false,
                ),
                (Err(e), _) => note(format!("Couldn't save the tray icon setting. {e}"), true),
            }
        }
    }
}

fn apply(ui: &AppWindow, v: view::View) {
    let g = ui.global::<Ui>();
    g.set_busy(false);
    g.set_sign_in(v.sign_in);
    g.set_sign_in_note(v.sign_in_note.into());
    g.set_last_run(v.last_run.into());
    g.set_tray_icon(v.tray_icon);
    g.set_ai_changes(v.ai_changes);
    g.set_notice(v.notice.into());
    g.set_notice_warn(v.notice_warn);
    g.set_screen(match v.screen {
        view::Screen::Devices => "devices".into(),
        view::Screen::Message => "message".into(),
    });
    g.set_chip(v.chip.into());
    g.set_chip_warn(v.chip_warn);
    g.set_heading(v.heading.into());
    g.set_message(v.message.into());
    g.set_summary(v.summary.into());
    g.set_can_set_everything(v.rows.iter().any(|r| r.can_color));
    let fresh: Vec<DeviceRow> = v
        .rows
        .into_iter()
        .map(|r| DeviceRow {
            name_key: r.name_key.into(),
            location_key: r.location_key.into(),
            status: r.status.as_str().into(),
            status_text: r.status_text.into(),
            can_keep: r.can_keep,
            dark: r.dark,
            has_before: r.has_before,
            can_color: r.can_color,
            can_color_zones: r.can_color_zones,
            zone_choices: ModelRc::new(VecModel::from(
                std::iter::once("Whole device".to_string())
                    .chain(r.zone_items.iter().map(|z| z.name.clone()))
                    .map(slint::SharedString::from)
                    .collect::<Vec<_>>(),
            )),
            zone_list: ModelRc::new(VecModel::from(
                r.zone_items
                    .iter()
                    .map(|z| ZoneItem {
                        name: z.name.clone().into(),
                        label: z.label.clone().into(),
                        lit: z.swatch.is_some(),
                        swatch: z.swatch.map_or(Color::from_rgb_u8(0, 0, 0), |(r, g, b)| Color::from_rgb_u8(r, g, b)),
                    })
                    .collect::<Vec<_>>(),
            )),
            own_lighting: r.own_lighting,
            lit_by: r.lit_by.into(),
            lit_by_warn: r.lit_by_warn,
            name: r.name.into(),
            product: r.product.into(),
            meta: r.meta.into(),
            zones: r.zones.into(),
            note: r.note.into(),
            lit: r.swatch.is_some(),
            swatch: r.swatch.map_or(Color::from_rgb_u8(0, 0, 0), |(r, g, b)| Color::from_rgb_u8(r, g, b)),
        })
        .collect();
    let devices = g.get_devices();
    if let Some(model) = devices.as_any().downcast_ref::<VecModel<DeviceRow>>() {
        sync(model, fresh);
    }
}

/// Updates the model in place, so focus and scroll survive the refresh.
fn sync(model: &VecModel<DeviceRow>, fresh: Vec<DeviceRow>) {
    if model.row_count() != fresh.len() {
        model.set_vec(fresh);
        return;
    }
    for (i, item) in fresh.into_iter().enumerate() {
        if !model.row_data(i).is_some_and(|old| same_row(&old, &item)) {
            model.set_row_data(i, item);
        }
    }
}

/// Whether two rows show the same. Slint compares its list models by identity,
/// and each refresh builds new ones, so the zone lists are compared by content:
/// a row that didn't change is left alone, and an open picker keeps its zone.
fn same_row(a: &DeviceRow, b: &DeviceRow) -> bool {
    fn items<T: Clone + 'static>(m: &ModelRc<T>) -> Vec<T> {
        m.iter().collect()
    }
    let strip =
        |r: &DeviceRow| DeviceRow { zone_choices: ModelRc::default(), zone_list: ModelRc::default(), ..r.clone() };
    strip(a) == strip(b)
        && items(&a.zone_choices) == items(&b.zone_choices)
        && items(&a.zone_list) == items(&b.zone_list)
}

/// Forces full repaints for a moment after the window is restored from
/// minimized. Slint's software renderer otherwise repaints only changed items
/// and leaves the rest of the restored window unpainted (found in Modlist
/// Launcher). Restoring passes through several sizes, so a few frames are forced.
fn repaint_after_restore(ui: &AppWindow) -> slint::Timer {
    const FORCED_FRAMES: u8 = 4;
    let weak = ui.as_weak();
    let mut was_minimized = false;
    let mut pending = 0u8;
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(150), move || {
        let Some(ui) = weak.upgrade() else { return };
        let minimized = window_hwnd(&ui).is_some_and(crate::platform::is_minimized);
        if was_minimized && !minimized {
            pending = FORCED_FRAMES;
        }
        was_minimized = minimized;
        if pending > 0 && !minimized {
            pending -= 1;
            ui.set_repaint_flip(!ui.get_repaint_flip());
            ui.window().request_redraw();
        }
    });
    timer
}

fn window_hwnd(ui: &AppWindow) -> Option<windows::Win32::Foundation::HWND> {
    let handle = ui.window().window_handle();
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(windows::Win32::Foundation::HWND(h.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// `keeplight-cli.exe`, installed beside `keeplight.exe`.
fn cli_path() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name("keeplight-cli.exe")
}

/// Each AI app's own command for adding Keeplight's MCP server, for the user
/// to run once. Keeplight never edits another app's settings.
fn connect_commands(cli: &std::path::Path) -> [String; 3] {
    let cli = cli.display();
    [
        format!("claude mcp add --scope user keeplight -- \"{cli}\" mcp"),
        format!("gemini mcp add --scope user keeplight \"{cli}\" mcp"),
        format!("codex mcp add keeplight -- \"{cli}\" mcp"),
    ]
}

#[cfg(test)]
mod tests {
    use super::connect_commands;

    #[test]
    fn connect_commands_quote_the_path_and_use_user_scope() {
        let [claude, gemini, codex] =
            connect_commands(std::path::Path::new(r"C:\Program Files\Keeplight\keeplight-cli.exe"));
        assert_eq!(
            claude,
            r#"claude mcp add --scope user keeplight -- "C:\Program Files\Keeplight\keeplight-cli.exe" mcp"#
        );
        assert_eq!(
            gemini,
            r#"gemini mcp add --scope user keeplight "C:\Program Files\Keeplight\keeplight-cli.exe" mcp"#
        );
        assert_eq!(codex, r#"codex mcp add keeplight -- "C:\Program Files\Keeplight\keeplight-cli.exe" mcp"#);
    }
}
