//! The `diagnose` bundle (part D): everything needed to understand a
//! problem in one JSON object, for the window's "Copy diagnostics", the
//! command line's `diagnose --json` and the MCP `diagnose` tool alike. It is
//! made to be pasted into a bug report or read by an AI, so every string in
//! it passes through [`Redactor`] first:
//!
//! - the home folder becomes `~` (any case, either path separator);
//! - values the app knows are secret (its keys, from the Keychain) become `[redacted]`;
//! - any value under a secret-looking key (`token`, `password`, `apiKey`,
//!   `authorization`, `cookie`, ...) becomes `[redacted]`;
//! - well-known credential shapes (`sk-ant-...`, `ghp_...`, `github_pat_...`,
//!   `Bearer ...`, JWTs, `AKIA...`) and `name=value` pairs with a secret-looking
//!   name become `[redacted]`;
//! - email addresses become `[email]`.
//!
//! Redaction is a backstop. The app still decides what goes in: counts and
//! states rather than records, never a file's contents, never a customer's
//! data.

use serde_json::Value;

use super::events::LogEvent;
use super::{KIT_VERSION, now_ms};

/// How many recent events a bundle includes.
pub const RECENT_EVENTS: usize = 200;

const REDACTED: &str = "[redacted]";
const EMAIL: &str = "[email]";
const SECRET_NAMES: [&str; 13] = [
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "authorization",
    "cookie",
    "privatekey",
    "credential",
    "session",
    "signature",
    "bearer",
    "refresh",
];
const SECRET_PREFIXES: [&str; 11] =
    ["sk-ant-", "sk-", "ghp_", "gho_", "ghu_", "ghs_", "github_pat_", "xoxb-", "xoxp-", "AKIA", "AIza"];

/// Whether a JSON key or a `name=` names a secret.
pub fn secret_name(name: &str) -> bool {
    let n: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    n == "key" || n == "auth" || n == "pat" || SECRET_NAMES.iter().any(|s| n.contains(s))
}

fn credential_shaped(word: &str) -> bool {
    let body_ok = |rest: &str| rest.len() >= 8 && rest.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if SECRET_PREFIXES.iter().any(|p| word.strip_prefix(p).is_some_and(body_ok)) {
        return true;
    }
    // A JWT: three base64url parts, the first starting with `eyJ`.
    let parts: Vec<&str> = word.split('.').collect();
    let base64url = |p: &&str| p.len() >= 4 && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    parts.len() == 3 && parts[0].starts_with("eyJ") && parts.iter().all(base64url)
}

fn email_shaped(word: &str) -> bool {
    let word = word.trim_end_matches(['.', ':']);
    match word.split_once('@') {
        Some((user, domain)) => {
            !user.is_empty()
                && !user.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        }
        None => false,
    }
}

/// `text` with the folder `home` written as `~`. ASCII case is ignored and
/// `/` matches `\`, byte by byte, so Windows paths written either way match.
pub fn tilde(text: &str, home: &str) -> String {
    let (t, p) = (text.as_bytes(), home.trim_end_matches(['/', '\\']).as_bytes());
    if p.len() < 4 {
        return text.to_string();
    }
    let same = |a: u8, b: u8| {
        let sep = |c: u8| c == b'/' || c == b'\\';
        a.eq_ignore_ascii_case(&b) || (sep(a) && sep(b))
    };
    let mut out = String::with_capacity(text.len());
    let (mut i, mut start) = (0, 0);
    while i + p.len() <= t.len() {
        if t[i..i + p.len()].iter().zip(p).all(|(&a, &b)| same(a, b))
            && text.is_char_boundary(i)
            && text.is_char_boundary(i + p.len())
        {
            out.push_str(&text[start..i]);
            out.push('~');
            i += p.len();
            start = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[start..]);
    out
}

/// What to take out of a diagnosis.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    /// The home folder, written as `~`.
    pub home: Option<String>,
    /// Secret values the app holds (its API keys, pairing keys), replaced
    /// wherever they appear. Values shorter than 8 characters are ignored.
    pub secrets: Vec<String>,
    /// Keep email addresses (off: they become `[email]`).
    pub keep_emails: bool,
}

impl Redactor {
    /// The current user's home folder, from `HOME` or `USERPROFILE`.
    pub fn from_env() -> Redactor {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())
            .filter(|h| std::path::Path::new(h).is_absolute());
        Redactor { home, ..Redactor::default() }
    }

    pub fn with_secret(mut self, secret: impl Into<String>) -> Redactor {
        self.secrets.push(secret.into());
        self
    }

    /// One string, redacted.
    pub fn text(&self, text: &str) -> String {
        let mut s = match &self.home {
            Some(home) => tilde(text, home),
            None => text.to_string(),
        };
        for secret in self.secrets.iter().filter(|s| s.len() >= 8) {
            s = s.replace(secret.as_str(), REDACTED);
        }
        let is_sep = |c: char| c.is_whitespace() || "\"'`,;()<>[]{}".contains(c);
        let mut out = String::with_capacity(s.len());
        let mut word = String::new();
        let mut after_bearer = false;
        let flush = |word: &mut String, out: &mut String, after_bearer: &mut bool| {
            if word.is_empty() {
                return;
            }
            let redacted = if *after_bearer && word.len() >= 8 { REDACTED.to_string() } else { self.word(word) };
            *after_bearer = word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("token");
            out.push_str(&redacted);
            word.clear();
        };
        for c in s.chars() {
            if is_sep(c) {
                flush(&mut word, &mut out, &mut after_bearer);
                out.push(c);
            } else {
                word.push(c);
            }
        }
        flush(&mut word, &mut out, &mut after_bearer);
        out
    }

    fn word(&self, word: &str) -> String {
        if credential_shaped(word) {
            return REDACTED.into();
        }
        if word.contains('=') {
            // `a=1&token=xyz`, `?key=...`, `password=...`: each pair by itself.
            let mut out = String::with_capacity(word.len());
            for piece in word.split_inclusive(['&', '?']) {
                let (body, tail) = match piece.char_indices().last() {
                    Some((at, c)) if c == '&' || c == '?' => (&piece[..at], &piece[at..]),
                    _ => (piece, ""),
                };
                match body.split_once('=') {
                    Some((name, value)) if secret_name(name) && !value.is_empty() => {
                        out.push_str(name);
                        out.push('=');
                        out.push_str(REDACTED);
                    }
                    Some((name, value)) => {
                        out.push_str(name);
                        out.push('=');
                        out.push_str(&self.word(value));
                    }
                    None => out.push_str(body),
                }
                out.push_str(tail);
            }
            return out;
        }
        if !self.keep_emails && email_shaped(word) {
            return EMAIL.into();
        }
        word.to_string()
    }

    /// A JSON value, redacted in place: every string, and every value under
    /// a secret-looking key.
    pub fn value(&self, value: &mut Value) {
        match value {
            Value::String(s) => *s = self.text(s),
            Value::Array(a) => a.iter_mut().for_each(|v| self.value(v)),
            Value::Object(o) => {
                for (k, v) in o.iter_mut() {
                    if secret_name(k) && !v.is_null() && !v.is_boolean() {
                        *v = Value::String(REDACTED.into());
                    } else {
                        self.value(v);
                    }
                }
            }
            _ => {}
        }
    }
}

/// A diagnosis: the app's own `sections` (detection, status, problems,
/// settings, last job, crash records) plus the newest events, redacted.
pub fn bundle(
    app: &str,
    app_version: &str,
    schema_version: u32,
    sections: Value,
    recent_events: &[LogEvent],
    redactor: &Redactor,
) -> Value {
    let mut v = serde_json::json!({
        "schemaVersion": schema_version,
        "kitVersion": KIT_VERSION,
        "app": app,
        "appVersion": app_version,
        "generatedAtMs": now_ms(),
        "os": std::env::consts::OS,
        "sections": sections,
        "recentEvents": recent_events,
    });
    redactor.value(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_home_folder_becomes_a_tilde_whatever_its_case() {
        let p = r"C:\Users\Jason";
        assert_eq!(tilde(r"C:\users\jason\AppData\x.log", p), r"~\AppData\x.log");
        assert_eq!(tilde(r"a C:\Users\Jason b C:\Users\Jason", p), "a ~ b ~");
        assert_eq!(tilde(r"D:\Games\HITMAN 3", p), r"D:\Games\HITMAN 3");
        assert_eq!(tilde("C:/Users/Jason/data", p), "~/data");
        assert_eq!(tilde("/Users/someone/Library/x", "/Users/someone/"), "~/Library/x");
        assert_eq!(tilde("/x", "/"), "/x");
    }

    #[test]
    fn secrets_credentials_and_emails_are_taken_out() {
        let r = Redactor {
            home: Some("/Users/someone".into()),
            secrets: vec!["hunter2-but-longer".into(), "short".into()],
            keep_emails: false,
        };
        let t = |s: &str| r.text(s);
        assert_eq!(t("key hunter2-but-longer in /Users/someone/x"), "key [redacted] in ~/x");
        assert_eq!(t("short stays"), "short stays");
        assert_eq!(t("got sk-ant-api03-abcdefghijk back"), "got [redacted] back");
        assert_eq!(t("remote ghp_0123456789abcdefABCDEF ok"), "remote [redacted] ok");
        assert_eq!(t("Authorization: Bearer abc.def-ghi_jkl"), "Authorization: Bearer [redacted]");
        assert_eq!(t("GET /cb?code=1&token=abcdef&x=2"), "GET /cb?code=1&token=[redacted]&x=2");
        assert_eq!(t("password=pa55word!"), "password=[redacted]");
        assert_eq!(t("jwt eyJhbGciOi.eyJzdWIiOi.c2lnbmF0dXJl"), "jwt [redacted]");
        assert_eq!(t("mail owner@example.com, now"), "mail [email], now");
        assert_eq!(t("version 1.2.3 at 12:00"), "version 1.2.3 at 12:00");
        let keep = Redactor { keep_emails: true, ..Redactor::default() };
        assert_eq!(keep.text("owner@example.com"), "owner@example.com");
    }

    #[test]
    fn values_under_secret_keys_are_redacted_whatever_their_type() {
        let r = Redactor::default();
        let mut v = serde_json::json!({
            "apiKey": "abc",
            "settings": { "refresh_token": { "nested": 1 }, "aiChanges": false, "hasKey": true, "count": 3 },
            "list": ["owner@example.com"],
            "sessionCookie": null
        });
        r.value(&mut v);
        assert_eq!(v["apiKey"], "[redacted]");
        assert_eq!(v["settings"]["refresh_token"], "[redacted]");
        assert_eq!(v["settings"]["aiChanges"], false);
        assert_eq!(v["settings"]["hasKey"], true, "a boolean only says whether a key is set");
        assert_eq!(v["settings"]["count"], 3);
        assert_eq!(v["list"][0], "[email]");
        assert!(v["sessionCookie"].is_null());
        assert!(secret_name("X-Api-Key") && secret_name("client_secret") && !secret_name("keyboard_layout"));
    }

    #[test]
    fn a_bundle_is_redacted_as_a_whole() {
        let r = Redactor { home: Some("/home/someone".into()), ..Redactor::default() };
        let b =
            bundle("app", "1.0.0", 2, serde_json::json!({ "dataFolder": "/home/someone/.app", "token": "t" }), &[], &r);
        assert_eq!(b["sections"]["dataFolder"], "~/.app");
        assert_eq!(b["sections"]["token"], "[redacted]");
        assert_eq!(b["schemaVersion"], 2);
        assert_eq!(b["kitVersion"], KIT_VERSION);
    }
}
