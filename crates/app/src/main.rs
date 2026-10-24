#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gui;
mod platform;

slint::include_modules!();

const USAGE: &str = "keeplight             Open Keeplight\nkeeplight --version   Show the version";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        None => gui::run(),
        Some("--version") => {
            show_message("Keeplight", &format!("Version {}", env!("CARGO_PKG_VERSION")));
            0
        }
        Some("--help" | "-h" | "/?") => {
            show_message("Keeplight", USAGE);
            0
        }
        Some(other) => {
            show_message("Keeplight", &format!("Unknown option {other}.\n\n{USAGE}"));
            2
        }
    };
    std::process::exit(code);
}

/// A plain message for errors before the main window exists, and for the
/// command line, which has no console in release builds.
pub fn show_message(heading: &str, message: &str) {
    let Ok(w) = MessageWindow::new() else { return };
    w.set_heading(heading.into());
    w.set_message(message.into());
    let weak = w.as_weak();
    w.on_ok(move || {
        if let Some(w) = weak.upgrade() {
            let _ = w.hide();
        }
    });
    let _ = w.run();
}
