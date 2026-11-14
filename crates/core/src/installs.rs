//! Which OpenRGB and PawnIO this PC has, against the versions Keeplight is
//! tested with (stage 05). Keeplight offers only tested versions (owner
//! decision 2026-11-14): a newer OpenRGB joins [`TESTED`] after a test on the
//! real PC, never automatically.
//!
//! Read from Windows, without admin rights: the installed programs list
//! (`...\CurrentVersion\Uninstall`) for the versions, and the service manager
//! for the OpenRGB service. Nothing is changed.

use serde::Serialize;

/// Something Keeplight depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Product {
    /// Does the hardware work and runs the SDK server Keeplight talks to.
    OpenRgb,
    /// The driver OpenRGB needs to reach the RAM and some motherboards.
    PawnIo,
}

impl Product {
    /// Its key in the window and the JSON: "openrgb" or "pawnio".
    pub fn key(self) -> &'static str {
        match self {
            Product::OpenRgb => "openrgb",
            Product::PawnIo => "pawnio",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Product::OpenRgb => "OpenRGB",
            Product::PawnIo => "PawnIO",
        }
    }

    /// The name in Windows' installed programs list.
    fn display_name(self) -> &'static str {
        self.name()
    }
}

/// How a pinned installer runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A Windows Installer package, run by `msiexec /i`.
    Msi,
    /// A setup program, run as it is.
    Exe,
}

/// A version Keeplight is tested with, and the official installer for it,
/// pinned by size and SHA-256. Keeplight installs nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pinned {
    pub product: Product,
    pub version: &'static str,
    /// Where the project itself publishes it. Always https.
    pub url: &'static str,
    /// The file name it is saved as.
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub kind: Kind,
}

/// The versions Keeplight is tested with, newest first per product.
/// Hashes taken 2026-11-14 from the official downloads (owner's OK); the
/// OpenRGB package is the one installed on JASON-PC (same product code),
/// and PawnIO's matches the SHA-256 GitHub publishes for the release.
pub const TESTED: &[Pinned] = &[
    // On JASON-PC since September 2026; every stage so far ran against it.
    Pinned {
        product: Product::OpenRgb,
        version: "1.0.0",
        url: "https://codeberg.org/OpenRGB/OpenRGB/releases/download/release_1.0/OpenRGB_1.0_Windows_64_81bbe18.msi",
        file: "OpenRGB_1.0_Windows_64_81bbe18.msi",
        size: 23_336_448,
        sha256: "27cd65546ea321eb61403825af494bb5caf2a212bafdbd55de7a061b0c3ef553",
        kind: Kind::Msi,
    },
    Pinned {
        product: Product::PawnIo,
        version: "2.2.0",
        url: "https://github.com/namazso/PawnIO.Setup/releases/download/2.2.0/PawnIO_setup.exe",
        file: "PawnIO_setup-2.2.0.exe",
        size: 3_410_960,
        sha256: "1f519a22e47187f70a1379a48ca604981c4fcf694f4e65b734aaa74a9fba3032",
        kind: Kind::Exe,
    },
];

/// How an installed version stands against the tested ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "version")]
pub enum Standing {
    NotInstalled,
    Tested(String),
    /// Older than every tested version.
    Older(String),
    /// Newer than the newest tested version, or between tested ones.
    NotTested(String),
}

impl Standing {
    pub fn is_tested(&self) -> bool {
        matches!(self, Standing::Tested(_))
    }
}

/// One product on this PC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub product: Product,
    pub standing: Standing,
    /// The newest tested version, which Keeplight would offer.
    pub tested: String,
}

/// Whether the OpenRGB service is there and running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Service {
    Running,
    Stopped,
    NotInstalled,
    /// Windows wouldn't say.
    Unknown,
}

/// What this PC has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installs {
    pub openrgb: Component,
    pub pawnio: Component,
    pub service: Service,
}

impl Installs {
    /// One line for the window, such as "OpenRGB 1.0.0 (tested), running as
    /// a service. PawnIO 2.2.0 (tested)."
    pub fn summary(&self) -> String {
        let part = |c: &Component| match &c.standing {
            Standing::NotInstalled => format!("{} isn't installed", c.product.name()),
            Standing::Tested(v) => format!("{} {v} (tested)", c.product.name()),
            Standing::Older(v) => format!("{} {v}, older than the tested {}", c.product.name(), c.tested),
            Standing::NotTested(v) => {
                format!("{} {v}, not tested yet (Keeplight is tested with {})", c.product.name(), c.tested)
            }
        };
        let service = match (&self.openrgb.standing, self.service) {
            (Standing::NotInstalled, _) => String::new(),
            (_, Service::Running) => ", running as a service".into(),
            (_, Service::Stopped) => ", but its service is stopped".into(),
            (_, Service::NotInstalled) => ", without its service".into(),
            (_, Service::Unknown) => String::new(),
        };
        format!("{}{service}. {}.", part(&self.openrgb), part(&self.pawnio))
    }

    /// Whether the window should show the line as a warning.
    pub fn needs_attention(&self) -> bool {
        !self.openrgb.standing.is_tested()
            || !self.pawnio.standing.is_tested()
            || matches!(self.service, Service::Stopped | Service::NotInstalled)
    }
}

/// Compares dotted versions by number: "1.0.0" equals "1.0.0.0", "1.10" is
/// newer than "1.9". Text that isn't a number counts as 0.
pub fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |v: &str| -> Vec<u64> { v.trim().split('.').map(|p| p.trim().parse().unwrap_or(0)).collect() };
    let (mut a, mut b) = (parts(a), parts(b));
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a.cmp(&b)
}

/// How `installed` stands against the tested versions of `product`.
pub fn assess(product: Product, installed: Option<&str>) -> Component {
    let tested: Vec<&str> = TESTED.iter().filter(|t| t.product == product).map(|t| t.version).collect();
    let newest = tested.iter().copied().max_by(|a, b| compare(a, b)).unwrap_or("none");
    let standing = match installed {
        None => Standing::NotInstalled,
        Some(v) if tested.iter().any(|t| compare(t, v).is_eq()) => Standing::Tested(v.to_string()),
        Some(v) if tested.iter().all(|t| compare(v, t).is_lt()) => Standing::Older(v.to_string()),
        Some(v) => Standing::NotTested(v.to_string()),
    };
    Component { product, standing, tested: newest.to_string() }
}

/// The installer Keeplight offers for `c`: the newest tested version, when
/// the product is missing or older. Never a downgrade, and nothing for an
/// untested newer version.
pub fn offer(c: &Component) -> Option<&'static Pinned> {
    if !matches!(c.standing, Standing::NotInstalled | Standing::Older(_)) {
        return None;
    }
    TESTED.iter().filter(|t| t.product == c.product).max_by(|a, b| compare(a.version, b.version))
}

impl Pinned {
    /// The host it downloads from, such as "codeberg.org".
    pub fn host(&self) -> &'static str {
        self.url.trim_start_matches("https://").split('/').next().unwrap_or(self.url)
    }

    /// Its size in whole megabytes, rounded up.
    pub fn megabytes(&self) -> u64 {
        self.size.div_ceil(1_000_000)
    }
}

impl Installs {
    /// What Keeplight would install first: OpenRGB, then PawnIO.
    pub fn next_offer(&self) -> Option<&'static Pinned> {
        offer(&self.openrgb).or_else(|| offer(&self.pawnio))
    }
}

/// What this PC has, read from Windows.
pub fn read() -> Installs {
    let version = |p: Product| windows_impl::installed_version(p.display_name());
    Installs {
        openrgb: assess(Product::OpenRgb, version(Product::OpenRgb).as_deref()),
        pawnio: assess(Product::PawnIo, version(Product::PawnIo).as_deref()),
        service: windows_impl::service("OpenRGB"),
    }
}

#[cfg(windows)]
mod windows_impl {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_SAM_FLAGS,
        RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
    };
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS,
        SERVICE_RUNNING, SERVICE_STATUS,
    };
    use windows::core::{HSTRING, PCWSTR, PWSTR};

    use super::Service;

    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

    fn string(key: HKEY, sub: &str, value: &str) -> Option<String> {
        let mut buf = [0u16; 512];
        let mut size = (buf.len() * 2) as u32;
        let status = unsafe {
            RegGetValueW(
                key,
                &HSTRING::from(sub),
                &HSTRING::from(value),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let len = (size as usize / 2).saturating_sub(1).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]).trim_end_matches('\0').to_string())
    }

    /// The DisplayVersion of the installed program named exactly `name`,
    /// from the machine's lists (64- and 32-bit) and the user's.
    pub fn installed_version(name: &str) -> Option<String> {
        let roots: [(HKEY, REG_SAM_FLAGS); 3] = [
            (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
            (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
            (HKEY_CURRENT_USER, REG_SAM_FLAGS(0)),
        ];
        for (root, view) in roots {
            let mut key = HKEY::default();
            if unsafe { RegOpenKeyExW(root, &HSTRING::from(UNINSTALL), None, KEY_READ | view, &mut key) }
                != ERROR_SUCCESS
            {
                continue;
            }
            let mut found = None;
            for i in 0.. {
                let mut sub = [0u16; 256];
                let mut len = sub.len() as u32;
                let status =
                    unsafe { RegEnumKeyExW(key, i, Some(PWSTR(sub.as_mut_ptr())), &mut len, None, None, None, None) };
                if status != ERROR_SUCCESS {
                    break;
                }
                let sub = String::from_utf16_lossy(&sub[..len as usize]);
                if string(key, &sub, "DisplayName").as_deref() == Some(name) {
                    found = string(key, &sub, "DisplayVersion");
                    break;
                }
            }
            unsafe {
                let _ = RegCloseKey(key);
            }
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// Whether the service named `name` exists and runs. Needs no admin.
    pub fn service(name: &str) -> Service {
        unsafe {
            let Ok(manager) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) else {
                return Service::Unknown;
            };
            let state = match OpenServiceW(manager, &HSTRING::from(name), SERVICE_QUERY_STATUS) {
                Ok(service) => {
                    let mut status = SERVICE_STATUS::default();
                    let state = match QueryServiceStatus(service, &mut status) {
                        Ok(()) if status.dwCurrentState == SERVICE_RUNNING => Service::Running,
                        Ok(()) => Service::Stopped,
                        Err(_) => Service::Unknown,
                    };
                    let _ = CloseServiceHandle(service);
                    state
                }
                Err(_) => Service::NotInstalled,
            };
            let _ = CloseServiceHandle(manager);
            state
        }
    }
}

#[cfg(not(windows))]
mod windows_impl {
    use super::Service;

    pub fn installed_version(_name: &str) -> Option<String> {
        None
    }

    pub fn service(_name: &str) -> Service {
        Service::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(compare("1.0.0", "1.0.0.0").is_eq());
        assert!(compare("1.10", "1.9").is_gt());
        assert!(compare("1.0", "1.1").is_lt());
        assert!(compare("2.2.0.0", "2.2.0").is_eq());
    }

    #[test]
    fn installed_versions_stand_against_the_tested_ones() {
        assert_eq!(assess(Product::OpenRgb, Some("1.0.0")).standing, Standing::Tested("1.0.0".into()));
        // Windows lists PawnIO as 2.2.0.0.
        assert!(assess(Product::PawnIo, Some("2.2.0.0")).standing.is_tested());
        assert_eq!(assess(Product::OpenRgb, Some("1.1")).standing, Standing::NotTested("1.1".into()));
        assert_eq!(assess(Product::OpenRgb, Some("0.9")).standing, Standing::Older("0.9".into()));
        assert_eq!(assess(Product::OpenRgb, None).standing, Standing::NotInstalled);
        assert_eq!(assess(Product::OpenRgb, None).tested, "1.0.0");
    }

    #[test]
    fn only_missing_or_older_products_are_offered_a_tested_installer() {
        let missing = offer(&assess(Product::OpenRgb, None)).unwrap();
        assert_eq!((missing.version, missing.kind), ("1.0.0", Kind::Msi));
        assert_eq!(offer(&assess(Product::PawnIo, Some("2.1.0"))).unwrap().version, "2.2.0");
        assert!(offer(&assess(Product::OpenRgb, Some("1.0.0"))).is_none());
        assert!(offer(&assess(Product::OpenRgb, Some("1.1"))).is_none(), "never a downgrade");
    }

    #[test]
    fn pinned_installers_are_https_with_a_full_sha256() {
        for t in TESTED {
            assert!(t.url.starts_with("https://"), "{}", t.url);
            assert_eq!(t.sha256.len(), 64);
            assert!(t.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert!(t.size > 0 && !t.file.contains(['/', '\\']));
        }
    }

    #[test]
    fn the_summary_says_what_needs_doing() {
        let both = |o: Option<&str>, p: Option<&str>, service| Installs {
            openrgb: assess(Product::OpenRgb, o),
            pawnio: assess(Product::PawnIo, p),
            service,
        };
        let ok = both(Some("1.0.0"), Some("2.2.0.0"), Service::Running);
        assert_eq!(ok.summary(), "OpenRGB 1.0.0 (tested), running as a service. PawnIO 2.2.0.0 (tested).");
        assert!(!ok.needs_attention());
        let newer = both(Some("1.1"), Some("2.2.0.0"), Service::Stopped);
        assert_eq!(
            newer.summary(),
            "OpenRGB 1.1, not tested yet (Keeplight is tested with 1.0.0), but its service is stopped. PawnIO \
             2.2.0.0 (tested)."
        );
        assert!(newer.needs_attention());
        let none = both(None, None, Service::NotInstalled);
        assert_eq!(none.summary(), "OpenRGB isn't installed. PawnIO isn't installed.");
    }

    /// On a Windows PC, reading works without admin and finds a version for
    /// whatever is installed. Not an assertion about this PC's contents.
    #[cfg(windows)]
    #[test]
    fn reading_this_pc_needs_no_admin() {
        let installs = read();
        assert!(!installs.summary().is_empty());
    }
}
