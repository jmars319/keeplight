//! The JAMARQ agent-interface kit, copied byte-identical into each app that
//! adopts it (`docs/agent-interfaces.md` in agentic-instructions). Do not
//! edit a copy: change the reference in the jamarq-agent-kit repository
//! (`rust/src/agent_kit`), then copy the folder and its record,
//! `agent-kit.sha256`, into each app.
//!
//! The app keeps everything that names it outside this folder: its error
//! codes, its data folder, its settings (including the `aiChanges` switch),
//! its tools and its server instructions.
//!
//! - [`error`]: typed errors with stable codes, and the JSON error report.
//! - [`gate`]: who is calling, and the rule that AI writes are off by default.
//! - [`plan`]: plans with a content hash, a 10-minute life and single use.
//! - [`journal`]: one JSON line per applied plan.
//! - [`events`]: the JSON Lines event log, a file per day, kept 14 days.
//! - [`diagnose`]: redaction for the `diagnose` bundle.
//! - [`mcp`]: helpers for a stdio MCP server built on `rmcp`.
//! - [`connect`]: finding Claude Code, Gemini CLI and Codex CLI, and the
//!   commands for a Connect button in Settings.

#![allow(dead_code)]

pub mod connect;
pub mod diagnose;
pub mod error;
pub mod events;
pub mod gate;
pub mod journal;
pub mod mcp;
pub mod plan;

use std::io::Write;
use std::path::Path;

use sha2::{Digest, Sha256};

/// The kit's version. Raised with every change to this folder, so a copy's
/// version says which reference it came from.
pub const KIT_VERSION: &str = "7";

/// Milliseconds since 1970 (UTC).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Writes `bytes` to `path` through a temporary file in the same folder and
/// one rename, so a reader never sees half a file. The temporary file is
/// removed when the write does not complete.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    let result = std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Appends one line to `path`, creating the file and its folder.
pub(crate) fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = String::with_capacity(line.len() + 1);
    text.push_str(line);
    text.push('\n');
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(text.as_bytes())
}

/// A folder under the system's temporary folder for one test, removed when
/// dropped. Keeps the kit free of a `tempfile` dependency.
#[cfg(test)]
pub(crate) struct TestDir(pub std::path::PathBuf);

#[cfg(test)]
impl TestDir {
    pub fn new(name: &str) -> TestDir {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("agent-kit-{name}-{}-{n}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        TestDir(dir)
    }
}

#[cfg(test)]
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The copy in this crate matches its record, `agent-kit.sha256` at the
    /// crate root. Fails when a copy was edited in place: change the
    /// reference in jamarq-agent-kit and copy it again instead.
    #[test]
    fn this_copy_matches_its_record() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let record = std::fs::read_to_string(root.join("agent-kit.sha256"))
            .expect("agent-kit.sha256 sits at the crate root beside src/agent_kit");
        let dir = root.join("src").join("agent_kit");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.ends_with(".rs"))
            .collect();
        on_disk.sort();
        let mut recorded = Vec::new();
        for line in record.lines().filter(|l| !l.trim().is_empty()) {
            let (hash, name) = line.split_once("  ").expect("each line is `<sha256>  <file>`");
            let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|_| panic!("{name} is in the record but missing"));
            assert_eq!(sha256_hex(&bytes), hash, "src/agent_kit/{name} differs from its record");
            recorded.push(name.to_string());
        }
        recorded.sort();
        assert_eq!(on_disk, recorded, "src/agent_kit holds exactly the recorded files");
    }

    #[test]
    fn atomic_writes_leave_no_temporary_file() {
        let tmp = TestDir::new("atomic");
        let path = tmp.0.join("a").join("x.json");
        write_atomic(&path, b"{}").unwrap();
        write_atomic(&path, b"[]").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"[]");
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        assert_eq!(sha256_hex(b"").len(), 64);
    }
}
