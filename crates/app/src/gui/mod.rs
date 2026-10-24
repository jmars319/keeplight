//! The main window. Stage 1 is read-only: it lists every light OpenRGB
//! reports, refreshing every few seconds, and changes none of them.

mod view;

use std::rc::Rc;
use std::time::Duration;

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

    // OpenRGB can take seconds to answer while it scans, so it is read on a
    // worker thread and only the result comes back to the window.
    let address = crate::platform::openrgb_address();
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        loop {
            let result = keeplight_core::sdk::snapshot(&address).map_err(|e| e.to_string());
            let v = view::build(&result, &address);
            if weak.upgrade_in_event_loop(move |ui| apply(&ui, v)).is_err() {
                return;
            }
            std::thread::sleep(REFRESH);
        }
    });

    let _repaint = repaint_after_restore(&ui);
    match ui.run() {
        Ok(()) => 0,
        Err(e) => {
            crate::show_message("Keeplight stopped", &e.to_string());
            1
        }
    }
}

fn apply(ui: &AppWindow, v: view::View) {
    let g = ui.global::<Ui>();
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
