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

#[cfg(windows)]
pub fn is_minimized(hwnd: windows::Win32::Foundation::HWND) -> bool {
    // SAFETY: plain window-state query.
    unsafe { windows::Win32::UI::WindowsAndMessaging::IsIconic(hwnd).as_bool() }
}
