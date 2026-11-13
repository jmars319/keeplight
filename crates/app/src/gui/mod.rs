//! The main window. It lists every light OpenRGB reports, refreshing every
//! few seconds. Keep saves what a device shows into the setup; Put back sends
//! a device its kept setup. Both run in `keeplight_core::keep`; this module
//! only passes requests to it and shows the results.

mod view;

use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use keeplight_core::keep::{self, Dark, HandBack, Outcome, Setup};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{Color, ComponentHandle, Model, ModelRc, VecModel};

use crate::{AppWindow, DeviceRow, Ui};

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
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let mut notice: Option<view::Notice> = None;
        loop {
            let result = keeplight_core::sdk::snapshot(&address).map_err(|e| e.to_string());
            let setup = Setup::load(&setup_path).map_err(|e| e.to_string());
            let mut v = view::build(&result, &setup, &address, notice.as_ref());
            let kept = setup.as_ref().map_or(0, |s| s.devices.len());
            let this_exe = crate::platform::startup::command_for_this_exe().ok();
            (v.sign_in, v.sign_in_note) =
                view::sign_in(crate::platform::startup::current().as_deref(), this_exe.as_deref(), kept);
            v.last_run = crate::platform::last_keeper_run().unwrap_or_default();
            v.tray_icon = keeplight_core::settings::Settings::load(&crate::platform::settings_path()).tray_icon;
            if weak.upgrade_in_event_loop(move |ui| apply(&ui, v)).is_err() {
                return;
            }
            match rx.recv_timeout(REFRESH) {
                Ok(command) => notice = Some(run_command(&command, &address, &setup_path)),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    let send = |make: fn(DeviceRow) -> Command| {
        let tx = tx.clone();
        let weak = ui.as_weak();
        move |row: DeviceRow| {
            if let Some(ui) = weak.upgrade() {
                ui.global::<Ui>().set_busy(true);
            }
            let _ = tx.send(make(row));
        }
    };
    g.on_keep(send(Command::Keep));
    g.on_put_back(send(Command::PutBack));
    g.on_keep_dark(send(Command::KeepDark));
    g.on_stop_keeping(send(Command::StopKeeping));
    g.on_keep_as_before(send(Command::KeepAsBefore));
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

    let _repaint = repaint_after_restore(&ui);
    match ui.run() {
        Ok(()) => 0,
        Err(e) => {
            crate::show_message("Keeplight stopped", &e.to_string());
            1
        }
    }
}

/// A button press, carrying the row it came from.
enum Command {
    Keep(DeviceRow),
    /// Keep a device whose lights are all off, after the row asked.
    KeepDark(DeviceRow),
    PutBack(DeviceRow),
    StopKeeping(DeviceRow),
    /// Keep a device again as it was before you stopped keeping it.
    KeepAsBefore(DeviceRow),
    /// Turn the keeper at sign-in on or off.
    SignIn(bool),
    /// Show or hide the keeper's icon in the notification area.
    TrayIcon(bool),
}

fn run_command(command: &Command, address: &str, setup_path: &Path) -> view::Notice {
    let note = |text: String, warn: bool| view::Notice { text, warn };
    match command {
        Command::Keep(row) => match keep::keep_now(address, setup_path, &row.name_key, &row.location_key, Dark::Refuse)
        {
            Ok(_) => note(format!("Kept {}. Put back returns it to what it shows now.", row.name), false),
            Err(e) => note(format!("Couldn't keep {}. {e}", row.name), true),
        },
        Command::KeepDark(row) => {
            match keep::keep_now(address, setup_path, &row.name_key, &row.location_key, Dark::Allow) {
                Ok(_) => note(format!("Kept {} dark. The keeper turns it back off after sleep.", row.name), false),
                Err(e @ keep::ActionError::NoLongerDark) => note(format!("Didn't keep {}. {e}", row.name), true),
                Err(e) => note(format!("Couldn't keep {}. {e}", row.name), true),
            }
        }
        Command::KeepAsBefore(row) => {
            match keep::keep_as_before(address, setup_path, &row.name_key, &row.location_key) {
                Ok(Outcome::Held) => note(format!("Keeping {} as before, and put it back.", row.name), false),
                Ok(Outcome::NotHeld) => note(
                    format!("Keeping {} as before, but it reads back as something else after being sent it.", row.name),
                    true,
                ),
                Ok(Outcome::Skipped(reason)) => {
                    note(format!("Keeping {} as before, but didn't send it: {reason}.", row.name), true)
                }
                Err(e) => note(format!("Couldn't keep {} as before. {e}", row.name), true),
            }
        }
        Command::StopKeeping(row) => match keep::stop_keeping(address, setup_path, &row.name_key, &row.location_key) {
            Ok(HandBack::OwnLighting(mode)) => {
                note(format!("Stopped keeping {}. It's back on its own {mode} lighting.", row.name), false)
            }
            Ok(HandBack::NotTaken(mode, now)) => note(
                format!(
                    "Stopped keeping {}. It was sent its own {mode} lighting but still reports {now}. If it's \
                         dark, set it in OpenRGB or unplug it and plug it back in.",
                    row.name
                ),
                true,
            ),
            Ok(HandBack::LeftAsIs) => note(
                format!("Stopped keeping {}. It keeps showing what it shows now until it loses power.", row.name),
                false,
            ),
            Ok(HandBack::NotListed) => note(
                format!("Stopped keeping {}. OpenRGB doesn't list it right now, so nothing was sent.", row.name),
                false,
            ),
            Err(e) => note(format!("Couldn't stop keeping {}. {e}", row.name), true),
        },
        Command::PutBack(row) => match keep::put_back_now(address, setup_path, &row.name_key, &row.location_key) {
            Ok(Outcome::Held) => note(format!("Put {} back.", row.name), false),
            Ok(Outcome::NotHeld) => note(
                format!(
                    "Sent {} its kept setup, but it reads back as something else. Another program may be setting it, or it can't show that setup.",
                    row.name
                ),
                true,
            ),
            Ok(Outcome::Skipped(reason)) => note(format!("Didn't change {}: {reason}.", row.name), true),
            Err(e) => note(format!("Couldn't put {} back. {e}", row.name), true),
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
            own_lighting: r.own_lighting,
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
fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, fresh: Vec<T>) {
    if model.row_count() != fresh.len() {
        model.set_vec(fresh);
        return;
    }
    for (i, item) in fresh.into_iter().enumerate() {
        if model.row_data(i).as_ref() != Some(&item) {
            model.set_row_data(i, item);
        }
    }
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
