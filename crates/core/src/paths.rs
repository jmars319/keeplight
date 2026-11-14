//! Where Keeplight keeps its files and finds OpenRGB, for every front end:
//! the window, the keeper, the command line and the MCP server.

use std::path::PathBuf;

/// Keeplight's data folder and the files in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub data: PathBuf,
}

impl Paths {
    /// `KEEPLIGHT_DATA` when set (tests and smoke tests point it at a
    /// throwaway folder), otherwise `%LOCALAPPDATA%\Keeplight`.
    pub fn from_env() -> Paths {
        let data =
            std::env::var_os("KEEPLIGHT_DATA").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
                let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
                base.join("Keeplight")
            });
        Paths { data }
    }

    pub fn setup(&self) -> PathBuf {
        self.data.join("setup.json")
    }

    pub fn settings(&self) -> PathBuf {
        self.data.join("settings.json")
    }

    /// The keeper's own log of its runs.
    pub fn keeper_log(&self) -> PathBuf {
        self.data.join("keeper.log")
    }

    /// Plans waiting to be applied (agent-interface kit).
    pub fn plans(&self) -> PathBuf {
        self.data.join("plans")
    }

    /// The event log, a file per day (agent-interface kit).
    pub fn logs(&self) -> PathBuf {
        self.data.join("logs")
    }
}

/// OpenRGB's SDK address: `KEEPLIGHT_OPENRGB` when set (a fake server in
/// tests), otherwise 127.0.0.1:6742.
pub fn openrgb_address() -> String {
    std::env::var("KEEPLIGHT_OPENRGB")
        .ok()
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| crate::sdk::DEFAULT_ADDRESS.to_string())
}

/// Whether an OpenRGB address is on this PC: a loopback host. Anything else
/// is another computer, whose lighting apps this PC can't see.
pub fn is_this_pc(address: &str) -> bool {
    use std::net::ToSocketAddrs;
    address.to_socket_addrs().map(|mut a| a.all(|a| a.ip().is_loopback())).unwrap_or(false)
}

/// The names of running programs, in lower case, such as "lghub_agent.exe",
/// for guessing which lighting app lights a device (`lit_by`). Reading the
/// list needs no admin rights; an empty list on failure only makes the guess
/// vaguer.
#[cfg(windows)]
pub fn running_programs() -> Vec<String> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let mut names = Vec::new();
    // SAFETY: a process snapshot read entry by entry, then closed.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return names };
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            names.push(String::from_utf16_lossy(&entry.szExeFile[..end]).to_lowercase());
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = windows::Win32::Foundation::CloseHandle(snapshot);
    }
    names
}

#[cfg(not(windows))]
pub fn running_programs() -> Vec<String> {
    Vec::new()
}

/// The running programs, when OpenRGB is on this PC; none otherwise.
pub fn running_programs_for(address: &str) -> Vec<String> {
    if is_this_pc(address) { running_programs() } else { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_addresses_are_this_pc() {
        assert!(is_this_pc("127.0.0.1:6742"));
        assert!(is_this_pc("localhost:6742"));
        assert!(is_this_pc("[::1]:6742"));
        assert!(!is_this_pc("192.168.1.20:6742"));
        assert!(!is_this_pc("not an address"));
    }

    #[test]
    fn files_live_in_the_data_folder() {
        let p = Paths { data: PathBuf::from("d") };
        assert_eq!(p.setup(), PathBuf::from("d").join("setup.json"));
        assert_eq!(p.logs(), PathBuf::from("d").join("logs"));
        assert_eq!(p.plans(), PathBuf::from("d").join("plans"));
    }
}
