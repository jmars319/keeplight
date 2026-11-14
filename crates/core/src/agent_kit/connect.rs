//! Connecting an AI app to this app's MCP server from a button in Settings,
//! in place of a command the owner pastes into a terminal (Vaexil Mod
//! Manager's design, 2026-11-14). The kit finds each AI app where it really
//! is and builds the exact command lines; the app shows each command in its
//! confirm step before running it, and keeps its own buttons, wording and
//! list of requests to try.
//!
//! Rules:
//! - Nothing found here runs until the owner has seen the exact command
//!   ([`CommandLine::display`]) and confirmed it.
//! - The app runs the AI app's own `mcp add`, `mcp remove` and status
//!   commands, and never writes another app's settings file itself.
//! - The test request costs the owner's plan or credits, so it is its own
//!   button with that warning, and never runs by itself. Where the found copy
//!   can't run headless (the copy inside the Claude desktop app is signed in
//!   only there), the app copies the request for the owner to paste instead,
//!   and watches for the call to arrive ([`super::mcp::record_client`]).
//! - No key or token passes through any of this.
//!
//! [`read_check`] turns each AI app's status output into one plain state.
//! Codex CLI's check only reads its settings ("Configured", not proof it
//! works), and Gemini CLI uses a server only in folders the person trusts.
//!
//! Where each AI app is looked for, in order (first found wins):
//!
//! | | Windows | macOS | Linux |
//! | --- | --- | --- | --- |
//! | all three | `PATH` | `PATH` | `PATH` |
//! | Claude Code | `~\.local\bin\claude.exe`, `%APPDATA%\npm\claude.cmd`, then the newest copy in the Claude desktop app (`%LOCALAPPDATA%\Claude-Data\*\claude-code\<version>\*\claude.exe`) | `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, then the newest copy in the Claude desktop app (`~/Library/Application Support/Claude/claude-code/<version>/*/claude.app/Contents/MacOS/claude`) | `~/.local/bin` |
//! | Gemini CLI, Codex CLI | `%APPDATA%\npm\<name>.cmd` | `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin` | `~/.local/bin` |
//!
//! An app started from the Finder or the Start menu gets a short `PATH`
//! (on macOS, no `/opt/homebrew/bin`), which is why the fixed places matter.

use std::path::{Path, PathBuf};

/// An AI app that can use this app's MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Assistant {
    ClaudeCode,
    GeminiCli,
    CodexCli,
}

impl Assistant {
    pub const ALL: [Assistant; 3] = [Assistant::ClaudeCode, Assistant::GeminiCli, Assistant::CodexCli];

    /// The name people know it by.
    pub fn name(self) -> &'static str {
        match self {
            Assistant::ClaudeCode => "Claude Code",
            Assistant::GeminiCli => "Gemini CLI",
            Assistant::CodexCli => "Codex CLI",
        }
    }

    /// The command's own name.
    pub fn program(self) -> &'static str {
        match self {
            Assistant::ClaudeCode => "claude",
            Assistant::GeminiCli => "gemini",
            Assistant::CodexCli => "codex",
        }
    }

    /// Where to send someone who doesn't have it.
    pub fn install_page(self) -> &'static str {
        match self {
            Assistant::ClaudeCode => "https://docs.claude.com/en/docs/claude-code/setup",
            Assistant::GeminiCli => "https://github.com/google-gemini/gemini-cli",
            Assistant::CodexCli => "https://github.com/openai/codex",
        }
    }
}

/// How an AI app was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Path,
    /// `~/.local/bin`, the native installers' place.
    LocalBin,
    /// npm's global folder (`%APPDATA%\npm` on Windows).
    Npm,
    /// `/opt/homebrew/bin` or `/usr/local/bin` on macOS.
    Homebrew,
    /// The copy inside the Claude desktop app. It shares the owner's Claude
    /// Code settings, but is signed in only inside the desktop app.
    ClaudeDesktop,
}

/// An AI app found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub assistant: Assistant,
    pub path: PathBuf,
    pub source: Source,
}

impl Found {
    /// Whether a headless test request can run with this copy.
    pub fn can_run_headless(&self) -> bool {
        self.source != Source::ClaudeDesktop
    }
}

/// The operating system the places belong to.
// `MacOs` ends with the enum's name, which clippy flags wherever the kit is a
// private module, as apps declare it. The name stays so no app code changes.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

/// Where to look: the parts of the environment discovery depends on. Tests
/// build one by hand; apps use [`Places::from_env`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    pub os: Os,
    pub path: Vec<PathBuf>,
    pub home: Option<PathBuf>,
    /// `%LOCALAPPDATA%` on Windows.
    pub local_app_data: Option<PathBuf>,
    /// `%APPDATA%` on Windows.
    pub app_data: Option<PathBuf>,
    /// Folders for `/opt/homebrew/bin` and `/usr/local/bin`; tests replace them.
    pub system_bins: Vec<PathBuf>,
}

impl Places {
    pub fn from_env() -> Places {
        let os = if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        };
        let var = |k: &str| std::env::var_os(k).map(PathBuf::from).filter(|p| p.is_absolute());
        Places {
            os,
            path: std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default(),
            home: var("HOME").or_else(|| var("USERPROFILE")),
            local_app_data: var("LOCALAPPDATA"),
            app_data: var("APPDATA"),
            system_bins: if os == Os::MacOs {
                vec![PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")]
            } else {
                Vec::new()
            },
        }
    }
}

/// Where `assistant` is on this machine, or `None` when it isn't installed.
pub fn find(assistant: Assistant, places: &Places) -> Option<Found> {
    let program = assistant.program();
    let found = |path: PathBuf, source: Source| Some(Found { assistant, path, source });
    let is_file = |p: &Path| p.is_file();
    let names: Vec<String> = match places.os {
        Os::Windows => vec![format!("{program}.exe"), format!("{program}.cmd")],
        _ => vec![program.to_string()],
    };
    for dir in &places.path {
        for name in &names {
            let p = dir.join(name);
            if is_file(&p) {
                return found(p, Source::Path);
            }
        }
    }
    if let Some(home) = &places.home {
        let p = home.join(".local").join("bin").join(&names[0]);
        if is_file(&p) {
            return found(p, Source::LocalBin);
        }
    }
    match places.os {
        Os::Windows => {
            if let Some(app_data) = &places.app_data {
                let p = app_data.join("npm").join(format!("{program}.cmd"));
                if is_file(&p) {
                    return found(p, Source::Npm);
                }
            }
            if assistant == Assistant::ClaudeCode {
                let root = places.local_app_data.as_ref()?;
                let mut best: Option<(Vec<u64>, PathBuf)> = None;
                for data in read_dirs(&root.join("Claude-Data")) {
                    for (version, exe) in bundled_copies(&data.join("claude-code"), &["claude.exe"]) {
                        if best.as_ref().is_none_or(|(v, _)| version > *v) {
                            best = Some((version, exe));
                        }
                    }
                }
                return best.and_then(|(_, p)| found(p, Source::ClaudeDesktop));
            }
        }
        Os::MacOs => {
            for dir in &places.system_bins {
                let p = dir.join(program);
                if is_file(&p) {
                    return found(p, Source::Homebrew);
                }
            }
            if assistant == Assistant::ClaudeCode {
                let root = places.home.as_ref()?.join("Library/Application Support/Claude/claude-code");
                let inner = ["claude.app", "Contents", "MacOS", "claude"];
                return bundled_copies(&root, &inner)
                    .into_iter()
                    .max_by(|a, b| a.0.cmp(&b.0))
                    .and_then(|(_, p)| found(p, Source::ClaudeDesktop));
            }
        }
        Os::Linux => {}
    }
    None
}

fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

fn file_name(p: &Path) -> String {
    p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string()
}

/// `<root>/<version>/<hash>/<inner...>` copies, with their parsed versions.
fn bundled_copies(root: &Path, inner: &[&str]) -> Vec<(Vec<u64>, PathBuf)> {
    let mut out = Vec::new();
    for version_dir in read_dirs(root) {
        let Some(version) = parse_version(&file_name(&version_dir)) else { continue };
        for hash_dir in read_dirs(&version_dir) {
            let exe = inner.iter().fold(hash_dir.clone(), |p, part| p.join(part));
            if exe.is_file() {
                out.push((version.clone(), exe));
            }
        }
    }
    out
}

fn parse_version(s: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = s.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

/// A command ready to show and run. Built here so the confirm step shows
/// exactly what runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl CommandLine {
    /// The command as the owner sees it in the confirm step.
    pub fn display(&self) -> String {
        let quote = |s: &str| {
            if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@\\".contains(c)) {
                s.to_string()
            } else {
                format!("\"{}\"", s.replace('"', "\\\""))
            }
        };
        std::iter::once(quote(&self.program.display().to_string()))
            .chain(self.args.iter().map(|a| quote(a)))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A process for it. A `.cmd` shim runs directly: Rust's standard library
    /// starts it through `cmd.exe` and escapes its arguments, and refuses
    /// ones it can't pass safely.
    pub fn to_command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.args);
        command
    }
}

/// Whether `name` is usable as an MCP server name: lowercase letters, digits
/// and hyphens, as every app's own name is (`vaexil-mod-manager`, `keeplight`).
pub fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
}

fn checked(name: &str) -> &str {
    assert!(valid_server_name(name), "an MCP server name is lowercase letters, digits and hyphens: {name}");
    name
}

/// Adds this app's server to the AI app, for the owner (every project).
/// `server` is this app's own MCP program and `server_args` its arguments
/// (usually `["mcp"]`).
pub fn connect_command(found: &Found, name: &str, server: &Path, server_args: &[&str]) -> CommandLine {
    let name = checked(name).to_string();
    let server = server.display().to_string();
    let rest = server_args.iter().map(|a| a.to_string());
    let args: Vec<String> = match found.assistant {
        Assistant::ClaudeCode => {
            ["mcp", "add", "--scope", "user", &name, "--", &server].into_iter().map(String::from).chain(rest).collect()
        }
        Assistant::GeminiCli => {
            let mut a: Vec<String> =
                ["mcp", "add", "--scope", "user", &name, &server].into_iter().map(String::from).collect();
            if server_args.iter().any(|s| s.starts_with('-')) {
                a.push("--".into());
            }
            a.extend(rest);
            a
        }
        Assistant::CodexCli => ["mcp", "add", &name, "--", &server].into_iter().map(String::from).chain(rest).collect(),
    };
    CommandLine { program: found.path.clone(), args }
}

/// Removes this app's server from the AI app.
pub fn disconnect_command(found: &Found, name: &str) -> CommandLine {
    let name = checked(name).to_string();
    let args: Vec<&str> = match found.assistant {
        Assistant::ClaudeCode | Assistant::GeminiCli => vec!["mcp", "remove", "--scope", "user", &name],
        Assistant::CodexCli => vec!["mcp", "remove", &name],
    };
    CommandLine { program: found.path.clone(), args: args.into_iter().map(String::from).collect() }
}

/// The AI app's own status check. Free: it uses no AI tokens. Claude Code's
/// `mcp get` and Gemini CLI's `mcp list` start this app's server and
/// handshake; Codex CLI's `mcp get` only reads its settings. Read the result
/// with [`read_check`].
pub fn check_command(found: &Found, name: &str) -> CommandLine {
    let name = checked(name).to_string();
    let args: Vec<&str> = match found.assistant {
        Assistant::ClaudeCode | Assistant::CodexCli => vec!["mcp", "get", &name],
        Assistant::GeminiCli => vec!["mcp", "list"],
    };
    CommandLine { program: found.path.clone(), args: args.into_iter().map(String::from).collect() }
}

/// What a status check says, in the words a Connect card shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The AI app started the server and it answered.
    Connected,
    /// Added and turned on, but this AI app's check doesn't start the server
    /// (Codex CLI), so it isn't proof it works: "Last used by" is.
    Configured,
    /// Gemini CLI has it, but uses it only in folders the person has trusted
    /// in Gemini CLI.
    NeedsTrust,
    /// Added but turned off in the AI app.
    Disabled,
    /// Not added to this AI app.
    NotAdded,
    /// The check ran and the server didn't answer; the AI app's own words.
    Failed(String),
}

/// Reads a [`check_command`]'s exit code and output (stdout and stderr
/// together). Formats checked 2026-11-14 against Claude Code 2.1.323 and
/// 2.1.325, Gemini CLI 0.68.0 and Codex CLI 0.169.0.
pub fn read_check(assistant: Assistant, name: &str, exit_code: i32, output: &str) -> CheckOutcome {
    let lower = output.to_lowercase();
    let first_line = || output.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
    match assistant {
        Assistant::ClaudeCode => {
            if lower.contains("no mcp server") {
                return CheckOutcome::NotAdded;
            }
            match lower.lines().find(|l| l.trim_start().starts_with("status:")) {
                Some(l) if l.contains("failed") || l.contains("disconnected") => CheckOutcome::Failed(
                    output.lines().find(|o| o.to_lowercase().contains("status:")).unwrap_or("").trim().to_string(),
                ),
                Some(l) if l.contains("connected") => CheckOutcome::Connected,
                Some(l) if l.contains("disabled") => CheckOutcome::Disabled,
                _ => CheckOutcome::Failed(first_line()),
            }
        }
        Assistant::GeminiCli => {
            let mine = output.lines().find(|l| {
                let t = l.trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).to_lowercase();
                t.starts_with(&format!("{name}:"))
            });
            match mine.map(str::to_lowercase) {
                None => CheckOutcome::NotAdded,
                Some(l) if l.trim_end().ends_with("- disabled") && lower.contains("untrusted") => {
                    CheckOutcome::NeedsTrust
                }
                Some(l) if l.trim_end().ends_with("- disabled") => CheckOutcome::Disabled,
                Some(l) if l.contains("disconnected") => CheckOutcome::Failed(mine.unwrap_or("").trim().to_string()),
                Some(l) if l.contains("connected") => CheckOutcome::Connected,
                Some(_) => CheckOutcome::Failed(mine.unwrap_or("").trim().to_string()),
            }
        }
        Assistant::CodexCli => {
            if lower.contains("no mcp server") {
                CheckOutcome::NotAdded
            } else if lower.contains("enabled: false") {
                CheckOutcome::Disabled
            } else if exit_code == 0 && lower.contains("enabled: true") {
                CheckOutcome::Configured
            } else {
                CheckOutcome::Failed(first_line())
            }
        }
    }
}

/// One headless request, allowed only `tool` on this app's server, for the
/// test button. `None` when this copy can't run headless, or the kit has not
/// verified a way to limit that AI app to one tool (Gemini CLI and Codex CLI
/// for now): then the app copies `prompt` for the owner to paste instead, and
/// waits for the call to arrive.
pub fn test_command(found: &Found, name: &str, tool: &str, prompt: &str) -> Option<CommandLine> {
    let name = checked(name);
    if !found.can_run_headless() || found.assistant != Assistant::ClaudeCode {
        return None;
    }
    let args = vec!["-p".into(), prompt.to_string(), "--allowedTools".into(), format!("mcp__{name}__{tool}")];
    Some(CommandLine { program: found.path.clone(), args })
}

#[cfg(test)]
mod tests {
    use super::super::TestDir;
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "").unwrap();
    }

    fn places(os: Os, root: &Path) -> Places {
        Places {
            os,
            path: vec![root.join("onpath")],
            home: Some(root.join("home")),
            local_app_data: Some(root.join("Local")),
            app_data: Some(root.join("Roaming")),
            system_bins: vec![root.join("homebrew")],
        }
    }

    #[test]
    fn windows_finds_the_claude_desktop_copy_newest_first_and_npm_shims() {
        let tmp = TestDir::new("connect-win");
        let p = places(Os::Windows, &tmp.0);
        assert_eq!(find(Assistant::ClaudeCode, &p), None);
        let old = tmp.0.join("Local/Claude-Data/abc/claude-code/2.1.9/aaa/claude.exe");
        let new = tmp.0.join("Local/Claude-Data/abc/claude-code/2.1.10/bbb/claude.exe");
        touch(&old);
        touch(&new);
        touch(&tmp.0.join("Local/Claude-Data/abc/claude-code/notes/x/claude.exe"));
        let f = find(Assistant::ClaudeCode, &p).unwrap();
        assert_eq!((f.path.clone(), f.source), (new, Source::ClaudeDesktop));
        assert!(!f.can_run_headless());

        touch(&tmp.0.join("Roaming/npm/claude.cmd"));
        assert_eq!(find(Assistant::ClaudeCode, &p).unwrap().source, Source::Npm);
        touch(&tmp.0.join("home/.local/bin/claude.exe"));
        assert_eq!(find(Assistant::ClaudeCode, &p).unwrap().source, Source::LocalBin);
        touch(&tmp.0.join("onpath/claude.cmd"));
        let on_path = find(Assistant::ClaudeCode, &p).unwrap();
        assert_eq!(on_path.source, Source::Path);
        assert!(on_path.can_run_headless());

        touch(&tmp.0.join("Roaming/npm/codex.cmd"));
        assert_eq!(find(Assistant::CodexCli, &p).unwrap().source, Source::Npm);
        assert_eq!(find(Assistant::GeminiCli, &p), None);
    }

    #[test]
    fn macos_finds_homebrew_then_the_desktop_app_bundle() {
        let tmp = TestDir::new("connect-mac");
        let p = places(Os::MacOs, &tmp.0);
        let bundled = tmp
            .0
            .join("home/Library/Application Support/Claude/claude-code/2.1.325/485f/claude.app/Contents/MacOS/claude");
        touch(&bundled);
        touch(
            &tmp.0.join(
                "home/Library/Application Support/Claude/claude-code/2.1.321/ddbd/claude.app/Contents/MacOS/claude",
            ),
        );
        assert_eq!(find(Assistant::ClaudeCode, &p).unwrap().path, bundled);
        touch(&tmp.0.join("homebrew/claude"));
        assert_eq!(find(Assistant::ClaudeCode, &p).unwrap().source, Source::Homebrew);
        assert_eq!(find(Assistant::GeminiCli, &p), None, "only Claude Code ships inside the desktop app");
    }

    #[test]
    fn commands_are_each_apps_own_and_shown_exactly() {
        let at = |assistant, path: &str| Found { assistant, path: PathBuf::from(path), source: Source::Path };
        let server = Path::new("C:\\Program Files\\VMM\\vmm.exe");
        let claude = at(Assistant::ClaudeCode, "claude");
        assert_eq!(
            connect_command(&claude, "vaexil-mod-manager", server, &["mcp"]).display(),
            r#"claude mcp add --scope user vaexil-mod-manager -- "C:\Program Files\VMM\vmm.exe" mcp"#
        );
        let gemini = at(Assistant::GeminiCli, "gemini");
        assert_eq!(
            connect_command(&gemini, "keeplight", Path::new("/a/kl"), &["mcp"]).args,
            ["mcp", "add", "--scope", "user", "keeplight", "/a/kl", "mcp"]
        );
        assert_eq!(connect_command(&gemini, "x", Path::new("/a/x"), &["--stdio"]).args[6], "--");
        let codex = at(Assistant::CodexCli, "codex");
        assert_eq!(
            connect_command(&codex, "x", Path::new("/a/x"), &["mcp"]).args,
            ["mcp", "add", "x", "--", "/a/x", "mcp"]
        );
        assert_eq!(disconnect_command(&claude, "x").args, ["mcp", "remove", "--scope", "user", "x"]);
        assert_eq!(disconnect_command(&codex, "x").args, ["mcp", "remove", "x"]);
        assert_eq!(check_command(&claude, "x").args, ["mcp", "get", "x"]);
        assert_eq!(check_command(&gemini, "x").args, ["mcp", "list"]);
        assert_eq!(check_command(&codex, "x").args, ["mcp", "get", "x"], "codex mcp list dumps every server");

        let test = test_command(&claude, "keeplight", "get_status", "What is Keeplight's status?").unwrap();
        assert_eq!(test.args, ["-p", "What is Keeplight's status?", "--allowedTools", "mcp__keeplight__get_status"]);
        let desktop = Found { source: Source::ClaudeDesktop, ..claude.clone() };
        assert_eq!(
            test_command(&desktop, "keeplight", "get_status", "x"),
            None,
            "the desktop copy isn't signed in headless"
        );
        assert_eq!(test_command(&gemini, "keeplight", "get_status", "x"), None);
        assert!(!valid_server_name("Bad Name") && !valid_server_name("-x") && valid_server_name("vaexil-mod-manager"));
    }

    #[test]
    fn check_output_reads_as_one_plain_state() {
        use Assistant::*;
        use CheckOutcome::*;
        let claude_ok = "atlas:\n  Scope: User config\n  Status: \u{2714} Connected\n  Type: stdio\n";
        assert_eq!(read_check(ClaudeCode, "atlas", 0, claude_ok), Connected);
        let claude_bad = "atlas:\n  Status: \u{2718} Failed to connect\n";
        assert_eq!(read_check(ClaudeCode, "atlas", 0, claude_bad), Failed("Status: \u{2718} Failed to connect".into()));
        let claude_missing = "No MCP server named \"x\". Configured servers: atlas";
        assert_eq!(read_check(ClaudeCode, "x", 1, claude_missing), NotAdded);

        let gemini = "Configured MCP servers:\n\n\u{2713} keeplight: /a/kl mcp (stdio) - Connected\n\u{2717} other: x (stdio) - Disconnected\n";
        assert_eq!(read_check(GeminiCli, "keeplight", 0, gemini), Connected);
        assert!(matches!(read_check(GeminiCli, "other", 0, gemini), Failed(_)));
        assert_eq!(read_check(GeminiCli, "absent", 0, gemini), NotAdded);
        let untrusted = "Warning: MCP servers are configured but disabled because this folder is untrusted.\n\u{25cb} keeplight: /a/kl mcp (stdio) - Disabled\n";
        assert_eq!(read_check(GeminiCli, "keeplight", 0, untrusted), NeedsTrust);

        let codex = "keeplight\n  enabled: true\n  transport: stdio\n  command: /a/kl\n";
        assert_eq!(read_check(CodexCli, "keeplight", 0, codex), Configured);
        assert_eq!(read_check(CodexCli, "x", 1, "Error: No MCP server named 'x' found."), NotAdded);
        assert_eq!(read_check(CodexCli, "keeplight", 0, &codex.replace("true", "false")), Disabled);
    }

    #[test]
    #[should_panic(expected = "lowercase letters")]
    fn a_server_name_that_could_be_misread_is_refused() {
        let f = Found { assistant: Assistant::CodexCli, path: PathBuf::from("codex"), source: Source::Path };
        let _ = disconnect_command(&f, "x & del");
    }
}
