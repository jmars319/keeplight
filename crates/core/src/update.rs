//! Whether a newer Keeplight is out (stage 05): the shared release feed's
//! `release.json` for Keeplight, which a published release points at its
//! installer. Keeplight only tells you; it never downloads or installs
//! itself (owner decision: notice only, no auto-update, releases unsigned).
//!
//! Fetched with Windows' own `curl.exe`, https only, at most 64 KB, when the
//! window opens and once a day while it stays open, unless you turn the
//! check off. Until Keeplight's first release the feed doesn't exist, which
//! is not an error: there is simply nothing newer.

use serde::Deserialize;

use crate::installer::{InstallError, curl_args, run_curl};
use crate::installs::compare;

/// Keeplight's manifest in JAMARQ-Digital-LLC/jamarq-releases.
pub const FEED: &str =
    "https://github.com/JAMARQ-Digital-LLC/jamarq-releases/releases/download/keeplight-latest/release.json";

/// Where a release's installer may be: the feed's own releases only.
const TRUSTED: &str = "https://github.com/JAMARQ-Digital-LLC/jamarq-releases/releases/download/keeplight-v";

/// The feed's manifest, as `publish/feed.mjs` writes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub version: String,
    /// The installer.
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub notes: Option<String>,
}

/// Reads a manifest, refusing one whose installer isn't in the feed's own
/// releases or whose version isn't MAJOR.MINOR.PATCH.
pub fn parse(text: &str) -> Result<Release, String> {
    let r: Release = serde_json::from_str(text).map_err(|e| format!("the feed isn't a release manifest: {e}"))?;
    let numbers = r.version.split('.').collect::<Vec<_>>();
    if numbers.len() != 3 || numbers.iter().any(|n| n.is_empty() || !n.chars().all(|c| c.is_ascii_digit())) {
        return Err(format!("the feed names version \u{201c}{}\u{201d}", r.version));
    }
    if !r.url.starts_with(TRUSTED) {
        return Err("the feed points outside Keeplight's own releases".into());
    }
    Ok(r)
}

/// The release, when it is newer than `current`.
pub fn newer(release: Release, current: &str) -> Option<Release> {
    compare(&release.version, current).is_gt().then_some(release)
}

/// Asks the feed whether a Keeplight newer than this one is out. `Ok(None)`
/// when there's nothing newer, including before the first release.
pub fn check() -> Result<Option<Release>, String> {
    check_at(FEED, "https", env!("CARGO_PKG_VERSION"))
}

fn check_at(url: &str, protocols: &str, current: &str) -> Result<Option<Release>, String> {
    match run_curl(curl_args(url, None, 64 * 1024, protocols)) {
        Ok(body) => newer(parse(&String::from_utf8_lossy(&body))?, current).map_or(Ok(None), |r| Ok(Some(r))),
        // No release yet: GitHub answers 404 for a missing release asset.
        Err(InstallError::Download(why)) if why.contains("404") => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::TcpListener;

    use super::*;

    const GOOD: &str = r#"{"version":"0.2.0","notes":"Keeplight 0.2.0.","pub_date":"2026-11-14T00:00:00Z","url":"https://github.com/JAMARQ-Digital-LLC/jamarq-releases/releases/download/keeplight-v0.2.0/keeplight-0.2.0-x64-setup.exe","sha256":"0000000000000000000000000000000000000000000000000000000000000000"}"#;

    fn serve(status: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/release.json", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut req = [0u8; 2048];
                let _ = std::io::Read::read(&mut s, &mut req);
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(body.as_bytes());
            }
        });
        url
    }

    #[test]
    fn the_feed_manifest_is_read_and_compared() {
        let r = parse(GOOD).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert!(newer(r.clone(), "0.1.0").is_some());
        assert!(newer(r.clone(), "0.2.0").is_none());
        assert!(newer(r, "0.10.0").is_none());
    }

    #[test]
    fn a_manifest_pointing_elsewhere_or_oddly_versioned_is_refused() {
        let elsewhere = GOOD.replace("JAMARQ-Digital-LLC/jamarq-releases", "someone/else");
        assert!(parse(&elsewhere).unwrap_err().contains("outside"));
        assert!(parse(&GOOD.replace("0.2.0\",\"notes", "0.2\",\"notes")).is_err());
        assert!(parse("<html>").is_err());
    }

    #[test]
    fn checking_finds_a_newer_release_and_a_missing_feed_is_nothing_newer() {
        let found = check_at(&serve("200 OK", GOOD), "http", "0.1.0").unwrap();
        assert_eq!(found.unwrap().version, "0.2.0");
        assert_eq!(check_at(&serve("200 OK", GOOD), "http", "0.2.0").unwrap(), None);
        assert_eq!(check_at(&serve("404 Not Found", ""), "http", "0.1.0").unwrap(), None, "no release yet");
        assert!(check_at(&serve("500 Internal Server Error", ""), "http", "0.1.0").is_err());
    }
}
