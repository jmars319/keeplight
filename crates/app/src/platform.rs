//! Process and Windows lookups, kept here so the rest of the app stays
//! testable.

/// `KEEPLIGHT_OPENRGB` names another `host:port` for OpenRGB's SDK server.
/// Development and smoke-test runs use it to point at a fake server instead
/// of the real lights.
pub fn openrgb_address() -> String {
    std::env::var("KEEPLIGHT_OPENRGB")
        .ok()
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| keeplight_core::sdk::DEFAULT_ADDRESS.to_string())
}

/// Where the setup you chose to keep is saved: `setup.json` in
/// `KEEPLIGHT_DATA` when set (tests and smoke tests point it at a throwaway
/// folder), otherwise in `%LOCALAPPDATA%\Keeplight`.
pub fn setup_path() -> std::path::PathBuf {
    let dir = std::env::var_os("KEEPLIGHT_DATA")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let base =
                std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
            base.join("Keeplight")
        });
    dir.join("setup.json")
}

#[cfg(windows)]
pub fn is_minimized(hwnd: windows::Win32::Foundation::HWND) -> bool {
    // SAFETY: plain window-state query.
    unsafe { windows::Win32::UI::WindowsAndMessaging::IsIconic(hwnd).as_bool() }
}
