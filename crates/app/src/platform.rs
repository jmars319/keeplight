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

/// The keeper's log, beside the setup file.
pub fn log_path() -> std::path::PathBuf {
    setup_path().with_file_name("keeper.log")
}

/// Appends lines to the keeper's log, keeping only its last 400 lines once it
/// passes 800, so it never grows without bound.
pub fn append_log(lines: &[String]) -> std::io::Result<()> {
    let path = log_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut all: Vec<String> =
        std::fs::read_to_string(&path).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default();
    all.extend(lines.iter().cloned());
    if all.len() > 800 {
        all.drain(..all.len() - 400);
    }
    std::fs::write(&path, all.join("\n") + "\n")
}

/// The last summary line the keeper logged (detail lines are indented).
pub fn last_keeper_run() -> Option<String> {
    let text = std::fs::read_to_string(log_path()).ok()?;
    text.lines().rev().find(|l| !l.starts_with(' ') && !l.is_empty()).map(str::to_string)
}

/// The local time as `YYYY-MM-DD HH:MM:SS`, for the log.
#[cfg(windows)]
pub fn local_timestamp() -> String {
    // SAFETY: plain clock query.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Holds a named mutex for the life of the process, so only one keeper runs
/// at a time. `None` when another process already holds it.
#[cfg(windows)]
pub struct SingleInstance(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl SingleInstance {
    pub fn acquire(name: &str) -> Option<Self> {
        use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
        let wide = wide(name);
        // SAFETY: the name is a NUL-terminated UTF-16 string that outlives the call.
        let handle = unsafe {
            windows::Win32::System::Threading::CreateMutexW(None, false, windows::core::PCWSTR(wide.as_ptr()))
        }
        .ok()?;
        // SAFETY: reads the error left by CreateMutexW just above.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: the handle was just opened by this process.
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
            return None;
        }
        Some(Self(handle))
    }
}

#[cfg(windows)]
impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value.
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// The per-user startup entry that runs the keeper at sign-in:
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `Keeplight`.
pub mod startup {
    #[cfg(windows)]
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };
    #[cfg(windows)]
    use windows::core::PCWSTR;

    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    /// The value's name: `Keeplight`, or `KEEPLIGHT_STARTUP_NAME` when set, so
    /// the smoke test can click the switch without touching the real entry.
    fn value_name() -> String {
        std::env::var("KEEPLIGHT_STARTUP_NAME").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "Keeplight".into())
    }

    /// The command this exe would register: itself, quoted, with `--keep`.
    pub fn command_for_this_exe() -> std::io::Result<String> {
        Ok(format!("\"{}\" --keep", std::env::current_exe()?.display()))
    }

    /// The registered command, if there is one.
    #[cfg(windows)]
    pub fn current() -> Option<String> {
        let (key, value) = (super::wide(KEY), super::wide(&value_name()));
        let mut size = 0u32;
        // SAFETY: a size query with valid NUL-terminated names.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
        };
        if status.is_err() || size == 0 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        // SAFETY: `buf` holds `size` bytes, as the call above reported.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status.is_err() {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    /// Registers `command`, or removes the entry when `None`.
    #[cfg(windows)]
    pub fn set(command: Option<&str>) -> std::io::Result<()> {
        let (key, value) = (super::wide(KEY), super::wide(&value_name()));
        let status = match command {
            Some(cmd) => {
                let data = super::wide(cmd);
                // SAFETY: `data` is a NUL-terminated UTF-16 string; the size counts its bytes.
                unsafe {
                    RegSetKeyValueW(
                        HKEY_CURRENT_USER,
                        PCWSTR(key.as_ptr()),
                        PCWSTR(value.as_ptr()),
                        REG_SZ.0,
                        Some(data.as_ptr().cast()),
                        (data.len() * 2) as u32,
                    )
                }
            }
            None => {
                // SAFETY: valid NUL-terminated names.
                let status =
                    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(value.as_ptr())) };
                if status == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
                    windows::Win32::Foundation::ERROR_SUCCESS
                } else {
                    status
                }
            }
        };
        status.ok().map_err(std::io::Error::other)
    }
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
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
