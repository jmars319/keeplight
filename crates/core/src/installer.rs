//! Installing a tested OpenRGB or PawnIO (stage 05): download the pinned
//! official installer, check it, and run it with Windows' admin prompt.
//!
//! - **Download:** Windows' own `curl.exe` from System32 (no new crate, no
//!   network code in Keeplight itself), https only, following the project's
//!   redirects, never more bytes than pinned. Saved under
//!   `%LOCALAPPDATA%\Keeplight\downloads`.
//! - **Check:** the size and SHA-256 must match [`Pinned`] exactly, or the
//!   file is deleted and nothing runs.
//! - **Run:** the official installer with its own window, through Windows'
//!   admin prompt (`runas`). Declining the prompt installs nothing.
//!
//! Only the window offers this. AI tools and the command line can't install
//! software.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::installs::{Kind, Pinned};

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("Windows' curl.exe wasn't found at {0}, so Keeplight can't download.")]
    NoCurl(PathBuf),
    #[error("The download failed: {0}")]
    Download(String),
    #[error(
        "The download didn't match what Keeplight is tested with ({detail}), so it was deleted and nothing was \
         installed."
    )]
    Mismatch { detail: String },
    #[error("Keeplight couldn't save the download: {0}")]
    Io(#[from] std::io::Error),
    #[error("You declined Windows' admin prompt, so nothing was installed.")]
    Declined,
    #[error("Windows couldn't start the installer: {0}")]
    Start(String),
}

/// How an installer run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finished {
    Done,
    /// Installed, and Windows needs a restart to finish.
    RestartNeeded,
    /// The installer was closed or cancelled.
    Cancelled,
    /// It ended with another exit code.
    Failed(u32),
}

/// Reads exit codes the way Windows Installer reports them.
pub fn finished(exit: u32) -> Finished {
    match exit {
        0 => Finished::Done,
        // ERROR_SUCCESS_REBOOT_REQUIRED, ERROR_SUCCESS_REBOOT_INITIATED.
        3010 | 1641 => Finished::RestartNeeded,
        // ERROR_INSTALL_USEREXIT, ERROR_CANCELLED.
        1602 | 1223 => Finished::Cancelled,
        other => Finished::Failed(other),
    }
}

/// Checks a downloaded file against `pinned`.
pub fn verify(path: &Path, pinned: &Pinned) -> Result<(), InstallError> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hash.update(&buf[..n]);
    }
    let got = hash.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>();
    if size != pinned.size {
        return Err(InstallError::Mismatch { detail: format!("{size} bytes, not {}", pinned.size) });
    }
    if got != pinned.sha256 {
        return Err(InstallError::Mismatch { detail: format!("SHA-256 {got}") });
    }
    Ok(())
}

/// Windows' own curl, by full path so nothing on PATH stands in for it.
fn curl() -> PathBuf {
    let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join("curl.exe")
}

/// The arguments for one download: fail on HTTP errors, follow redirects,
/// stay on `protocols`, and stop past the pinned size.
fn curl_args(url: &str, out: &Path, max: u64, protocols: &str) -> Vec<String> {
    vec![
        "--fail".into(),
        "--silent".into(),
        "--show-error".into(),
        "--location".into(),
        "--proto".into(),
        format!("={protocols}"),
        "--proto-redir".into(),
        format!("={protocols}"),
        "--max-filesize".into(),
        max.to_string(),
        "--max-time".into(),
        "600".into(),
        "--output".into(),
        out.display().to_string(),
        url.into(),
    ]
}

/// Downloads `pinned` into `dir` and checks it. Returns the checked file.
pub fn download(pinned: &Pinned, dir: &Path) -> Result<PathBuf, InstallError> {
    download_from(pinned, pinned.url, dir, "https")
}

fn download_from(pinned: &Pinned, url: &str, dir: &Path, protocols: &str) -> Result<PathBuf, InstallError> {
    let curl = curl();
    if !curl.is_file() {
        return Err(InstallError::NoCurl(curl));
    }
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(pinned.file);
    let part = dir.join(format!("{}.part", pinned.file));
    let _ = std::fs::remove_file(&part);
    let mut command = std::process::Command::new(&curl);
    command.args(curl_args(url, &part, pinned.size, protocols));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: the window is a GUI program; no console flashes.
        command.creation_flags(0x0800_0000);
    }
    let output = command.output()?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&part);
        let why = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(InstallError::Download(if why.is_empty() {
            format!("curl exited with {}", output.status)
        } else {
            why
        }));
    }
    if let Err(e) = verify(&part, pinned) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&part, &dest)?;
    Ok(dest)
}

/// The program and arguments that run a checked installer.
fn command_for(path: &Path, kind: Kind) -> (String, String) {
    match kind {
        Kind::Msi => ("msiexec.exe".into(), format!("/i \"{}\"", path.display())),
        Kind::Exe => (path.display().to_string(), String::new()),
    }
}

/// Runs a checked installer with Windows' admin prompt, waits for it, and
/// reports how it ended. The file is checked again first, so nothing can
/// swap it between download and run.
pub fn run(path: &Path, pinned: &Pinned) -> Result<Finished, InstallError> {
    verify(path, pinned)?;
    let (program, args) = command_for(path, pinned.kind);
    elevated::run(&program, &args).map(finished)
}

/// Downloads, checks and runs the installer for `pinned`.
pub fn install(pinned: &Pinned, downloads: &Path) -> Result<Finished, InstallError> {
    let path = download(pinned, downloads)?;
    run(&path, pinned)
}

#[cfg(windows)]
mod elevated {
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
    use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
    use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR, w};

    use super::InstallError;

    /// Starts `program` as administrator (Windows asks first) and waits.
    pub fn run(program: &str, args: &str) -> Result<u32, InstallError> {
        let file = HSTRING::from(program);
        let params = HSTRING::from(args);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: if args.is_empty() { PCWSTR::null() } else { PCWSTR(params.as_ptr()) },
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe {
            if let Err(e) = ShellExecuteExW(&mut info) {
                return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
                    InstallError::Declined
                } else {
                    InstallError::Start(e.message())
                });
            }
            if info.hProcess.is_invalid() {
                return Err(InstallError::Start("Windows gave no process to wait for".into()));
            }
            WaitForSingleObject(info.hProcess, INFINITE);
            let mut code = 0u32;
            let got = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
            got.map_err(|e| InstallError::Start(e.message()))?;
            Ok(code)
        }
    }
}

#[cfg(not(windows))]
mod elevated {
    use super::InstallError;

    pub fn run(_program: &str, _args: &str) -> Result<u32, InstallError> {
        Err(InstallError::Start("installing needs Windows".into()))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::TcpListener;

    use super::*;
    use crate::installs::Product;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("keeplight-installer-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn pinned_for(body: &[u8]) -> Pinned {
        let sha: String = Sha256::digest(body).iter().map(|b| format!("{b:02x}")).collect();
        Pinned {
            product: Product::OpenRgb,
            version: "1.0.0",
            url: "https://example.invalid/",
            file: "test-setup.msi",
            size: body.len() as u64,
            sha256: Box::leak(sha.into_boxed_str()),
            kind: Kind::Msi,
        }
    }

    /// A one-request HTTP server on 127.0.0.1 answering `status` with `body`.
    fn serve(status: &'static str, body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/setup", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut req = [0u8; 2048];
                let _ = std::io::Read::read(&mut s, &mut req);
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        url
    }

    #[test]
    fn a_download_that_matches_is_kept() {
        let body = b"pretend installer".to_vec();
        let pinned = pinned_for(&body);
        let d = dir("ok");
        let path = download_from(&pinned, &serve("200 OK", body.clone()), &d, "http").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert!(!d.join("test-setup.msi.part").exists());
    }

    #[test]
    fn a_download_that_differs_is_deleted() {
        let pinned = pinned_for(b"the real one");
        let d = dir("mismatch");
        let err = download_from(&pinned, &serve("200 OK", b"the fake one".to_vec()), &d, "http").unwrap_err();
        assert!(matches!(err, InstallError::Mismatch { .. }), "{err}");
        assert!(std::fs::read_dir(&d).unwrap().next().is_none(), "nothing left behind");
    }

    #[test]
    fn a_download_larger_than_pinned_or_failing_is_refused() {
        let pinned = pinned_for(b"tiny");
        let d = dir("big");
        let err = download_from(&pinned, &serve("200 OK", vec![b'x'; 4096]), &d, "http").unwrap_err();
        assert!(matches!(err, InstallError::Download(_) | InstallError::Mismatch { .. }), "{err}");
        let err = download_from(&pinned, &serve("404 Not Found", Vec::new()), &dir("404"), "http").unwrap_err();
        assert!(matches!(err, InstallError::Download(_)), "{err}");
    }

    #[test]
    fn only_https_is_allowed_for_real_downloads() {
        let args = curl_args("https://codeberg.org/x.msi", Path::new("out"), 10, "https");
        assert!(args.windows(2).any(|w| w == ["--proto", "=https"]));
        assert!(args.windows(2).any(|w| w == ["--proto-redir", "=https"]));
        assert!(args.windows(2).any(|w| w == ["--max-filesize", "10"]));
        // A plain-http link is refused by curl itself.
        let pinned = pinned_for(b"x");
        let err = download(&Pinned { url: "http://127.0.0.1:9/x", ..pinned }, &dir("http")).unwrap_err();
        assert!(matches!(err, InstallError::Download(_)), "{err}");
    }

    #[test]
    fn installers_run_the_right_way_and_exit_codes_read_plainly() {
        let (p, a) = command_for(Path::new(r"C:\d\OpenRGB.msi"), Kind::Msi);
        assert_eq!((p.as_str(), a.as_str()), ("msiexec.exe", r#"/i "C:\d\OpenRGB.msi""#));
        let (p, a) = command_for(Path::new(r"C:\d\PawnIO_setup.exe"), Kind::Exe);
        assert_eq!((p.as_str(), a.as_str()), (r"C:\d\PawnIO_setup.exe", ""));
        assert_eq!(finished(0), Finished::Done);
        assert_eq!(finished(3010), Finished::RestartNeeded);
        assert_eq!(finished(1602), Finished::Cancelled);
        assert_eq!(finished(1603), Finished::Failed(1603));
    }

    /// The real pinned downloads still match (network; run by hand with
    /// `cargo test -p keeplight-core real_pinned -- --ignored`).
    #[test]
    #[ignore]
    fn real_pinned_downloads_match() {
        let d = dir("real");
        for t in crate::installs::TESTED {
            download(t, &d).unwrap_or_else(|e| panic!("{}: {e}", t.file));
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
