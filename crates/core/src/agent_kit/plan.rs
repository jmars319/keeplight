//! Plan, then apply (part P). Every change an agent can trigger is planned
//! first, by the same code that will write it. A plan records what it will
//! do (the app's payload), the target it is for, and the SHA-256 of every
//! file or record it depends on, and is saved in the app's `plans` folder
//! under a 16-hex-digit id. Its content hash covers all three, so an edited
//! plan file is refused.
//!
//! Applying it ([`PlanStore::take`]) needs an [`Authorized`] write, then
//! refuses when the plan is unknown, expired (10 minutes), for another
//! target, or when anything it depends on has changed; the app's own check
//! then recomputes the plan and compares. Only then is the plan claimed, by
//! one rename, so it is applied once even when two processes race.
//!
//! Plans past their time are deleted when the next plan is saved.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::error::{AgentError, Result, codes};
use super::gate::Authorized;
use super::{now_ms, sha256_hex, write_atomic};

/// How long a plan can be applied for.
pub const PLAN_LIFETIME_MS: u64 = 10 * 60 * 1000;

/// What a precondition names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PreconditionKind {
    /// A file, by its path. The kit hashes it again on apply.
    File,
    /// A record the app understands (a row, an entry, a remote object), by
    /// its key. The app hashes it again on apply.
    Record,
}

/// Something a plan depends on, as it was when the plan was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Precondition {
    pub kind: PreconditionKind,
    /// The file's path or the record's key.
    pub key: String,
    /// SHA-256 of its contents then, or `None` when it did not exist.
    pub sha256: Option<String>,
}

impl Precondition {
    /// A file as it is now.
    pub fn file(path: &Path) -> Result<Precondition> {
        Ok(Precondition { kind: PreconditionKind::File, key: path.display().to_string(), sha256: file_sha256(path)? })
    }

    /// A record as it is now: `bytes` is a stable serialization of it, or
    /// `None` when it does not exist.
    pub fn record(key: impl Into<String>, bytes: Option<&[u8]>) -> Precondition {
        Precondition { kind: PreconditionKind::Record, key: key.into(), sha256: bytes.map(sha256_hex) }
    }
}

/// SHA-256 of a file's contents, or `None` when it does not exist.
pub fn file_sha256(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(sha256_hex(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AgentError::new(codes::IO, format!("Couldn't read {}: {e}", path.display()))),
    }
}

/// For an app whose plans depend only on files: refuses a record
/// precondition, which it could not check.
pub fn files_only(key: &str) -> Result<Option<String>> {
    Err(AgentError::new(
        codes::STORE_UNREADABLE,
        format!("The plan depends on the record {key}, which this app does not check. Nothing was changed."),
    ))
}

/// A checked change, waiting to be applied by its `planId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Plan<P> {
    /// The app's own schema version.
    pub schema_version: u32,
    /// The handle to apply it with. Used once.
    pub plan_id: String,
    /// What it is for: a folder, a book, an account, as the app names it.
    pub target: String,
    /// The app's changes and the steps it will take, in plain words.
    pub payload: P,
    /// Applying refuses if any of these has changed since.
    pub preconditions: Vec<Precondition>,
    /// SHA-256 over the target, the payload and the preconditions.
    pub content_hash: String,
    pub created_at_ms: u64,
    /// After this, the plan can't be applied; plan again.
    pub expires_at_ms: u64,
}

/// SHA-256 of `{"payload":..,"preconditions":..,"target":..}` as canonical
/// JSON (object keys sorted, no spaces), the same in the Rust and TypeScript
/// kits and whatever `serde_json` features the app enables.
fn content_hash(target: &str, payload: &serde_json::Value, preconditions: &[Precondition]) -> String {
    let content = serde_json::json!({ "target": target, "payload": payload, "preconditions": preconditions });
    let mut text = String::new();
    canonical(&content, &mut text);
    sha256_hex(text.as_bytes())
}

/// JSON with every object's keys in sorted order and no whitespace.
pub fn canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("a key serializes"));
                out.push(':');
                canonical(&map[k], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(v, out);
            }
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).expect("a scalar serializes")),
    }
}

/// A plan id is 16 lowercase hex digits; nothing else names a plan file.
pub fn valid_plan_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn not_found(plan_id: &str) -> AgentError {
    AgentError::new(
        codes::PLAN_NOT_FOUND,
        format!("There is no plan \u{201c}{plan_id}\u{201d}. It may have been applied already."),
    )
}

/// The app's `plans` folder.
#[derive(Debug, Clone)]
pub struct PlanStore {
    dir: PathBuf,
    schema_version: u32,
    lifetime_ms: u64,
}

impl PlanStore {
    pub fn new(dir: impl Into<PathBuf>, schema_version: u32) -> PlanStore {
        PlanStore { dir: dir.into(), schema_version, lifetime_ms: PLAN_LIFETIME_MS }
    }

    /// Only for tests: the fleet's plans live 10 minutes.
    pub fn with_lifetime_ms(mut self, lifetime_ms: u64) -> PlanStore {
        self.lifetime_ms = lifetime_ms;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, plan_id: &str) -> PathBuf {
        self.dir.join(format!("{plan_id}.json"))
    }

    /// Saves a plan. Making a plan changes nothing else.
    pub fn save<P: Serialize + DeserializeOwned>(
        &self,
        target: &str,
        payload: P,
        preconditions: Vec<Precondition>,
    ) -> Result<Plan<P>> {
        self.save_at(target, payload, preconditions, now_ms())
    }

    pub fn save_at<P: Serialize + DeserializeOwned>(
        &self,
        target: &str,
        payload: P,
        preconditions: Vec<Precondition>,
        now: u64,
    ) -> Result<Plan<P>> {
        let value = serde_json::to_value(&payload).expect("a plan payload serializes");
        let hash = content_hash(target, &value, &preconditions);
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| AgentError::new(codes::SAVE_FAILED, format!("Couldn't create {}: {e}", self.dir.display())))?;
        self.prune(now);
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let plan_id = loop {
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let seed = format!("{hash}:{now}:{}:{n}:{:?}", std::process::id(), std::time::Instant::now());
            let id = sha256_hex(seed.as_bytes())[..16].to_string();
            if !self.path(&id).exists() {
                break id;
            }
        };
        let plan = Plan {
            schema_version: self.schema_version,
            plan_id,
            target: target.to_string(),
            payload: value,
            preconditions,
            content_hash: hash,
            created_at_ms: now,
            expires_at_ms: now + self.lifetime_ms,
        };
        let bytes = serde_json::to_vec_pretty(&plan).expect("a plan serializes");
        write_atomic(&self.path(&plan.plan_id), &bytes)
            .map_err(|e| AgentError::new(codes::SAVE_FAILED, format!("Couldn't save the plan: {e}")))?;
        Ok(Plan {
            schema_version: plan.schema_version,
            plan_id: plan.plan_id,
            target: plan.target,
            payload,
            preconditions: plan.preconditions,
            content_hash: plan.content_hash,
            created_at_ms: plan.created_at_ms,
            expires_at_ms: plan.expires_at_ms,
        })
    }

    /// Reads a saved plan without claiming it, for a command line that finds
    /// the target from the plan. Checks its content hash, not its age.
    pub fn peek<P: DeserializeOwned>(&self, plan_id: &str) -> Result<Plan<P>> {
        if !valid_plan_id(plan_id) {
            return Err(not_found(plan_id));
        }
        let path = self.path(plan_id);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(not_found(plan_id)),
            Err(e) => return Err(AgentError::new(codes::IO, format!("Couldn't read {}: {e}", path.display()))),
        };
        let unreadable = |e: String| {
            AgentError::new(codes::STORE_UNREADABLE, format!("The plan file {} can't be read: {e}", path.display()))
        };
        let plan: Plan<serde_json::Value> = serde_json::from_slice(&bytes).map_err(|e| unreadable(e.to_string()))?;
        if content_hash(&plan.target, &plan.payload, &plan.preconditions) != plan.content_hash {
            return Err(AgentError::new(
                codes::CHANGED_MEANWHILE,
                "The plan file was edited after the plan was made. Nothing was changed.",
            ));
        }
        let payload = serde_json::from_value(plan.payload).map_err(|e| unreadable(e.to_string()))?;
        Ok(Plan {
            schema_version: plan.schema_version,
            plan_id: plan.plan_id,
            target: plan.target,
            payload,
            preconditions: plan.preconditions,
            content_hash: plan.content_hash,
            created_at_ms: plan.created_at_ms,
            expires_at_ms: plan.expires_at_ms,
        })
    }

    /// Claims a plan for applying, once, after every check:
    ///
    /// 1. `write` came from [`super::gate::authorize_write`];
    /// 2. the plan exists, reads, and its content hash holds;
    /// 3. it has not expired (an expired plan is deleted);
    /// 4. it is for `target`;
    /// 5. every file precondition hashes the same now, and every record
    ///    precondition hashes the same through `record_sha256`;
    /// 6. `verify`, the app's own check, passes: it should recompute the
    ///    plan from the state as it is now and refuse with
    ///    `plan.changed_meanwhile` when the steps differ.
    ///
    /// Returns the plan, now removed from the store. The app then applies
    /// it through its normal write path and appends it to the journal.
    pub fn take<P: DeserializeOwned>(
        &self,
        write: &Authorized,
        plan_id: &str,
        target: &str,
        record_sha256: impl Fn(&str) -> Result<Option<String>>,
        verify: impl FnOnce(&Plan<P>) -> Result<()>,
    ) -> Result<Plan<P>> {
        self.take_at(write, plan_id, target, record_sha256, verify, now_ms())
    }

    pub fn take_at<P: DeserializeOwned>(
        &self,
        _write: &Authorized,
        plan_id: &str,
        target: &str,
        record_sha256: impl Fn(&str) -> Result<Option<String>>,
        verify: impl FnOnce(&Plan<P>) -> Result<()>,
        now: u64,
    ) -> Result<Plan<P>> {
        let plan: Plan<P> = self.peek(plan_id)?;
        if now > plan.expires_at_ms {
            let _ = std::fs::remove_file(self.path(plan_id));
            return Err(AgentError::new(
                codes::PLAN_EXPIRED,
                "That plan is more than 10 minutes old. Nothing was changed.",
            ));
        }
        if plan.target != target {
            return Err(AgentError::new(
                codes::PLAN_OTHER_TARGET,
                format!("That plan is for {}, not {target}. Nothing was changed.", plan.target),
            ));
        }
        for p in &plan.preconditions {
            let now_hash = match p.kind {
                PreconditionKind::File => file_sha256(Path::new(&p.key))?,
                PreconditionKind::Record => record_sha256(&p.key)?,
            };
            if now_hash != p.sha256 {
                return Err(AgentError::new(
                    codes::CHANGED_MEANWHILE,
                    format!("{} has changed since the plan was made. Nothing was changed.", p.key),
                ));
            }
        }
        verify(&plan)?;
        // Claim it by one rename: of two processes applying the same plan,
        // only one finds the file.
        let claimed = self.dir.join(format!("{plan_id}.json.claimed-{}", std::process::id()));
        match std::fs::rename(self.path(plan_id), &claimed) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(not_found(plan_id)),
            Err(e) => {
                return Err(AgentError::new(
                    codes::SAVE_FAILED,
                    format!("Couldn't mark the plan as used ({e}). Nothing was changed."),
                ));
            }
        }
        let _ = std::fs::remove_file(&claimed);
        Ok(plan)
    }

    /// Deletes plans that can no longer be applied, and leftovers of claims
    /// and writes. Best effort.
    fn prune(&self, now: u64) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        for e in entries.flatten() {
            let path = e.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let leftover = name.contains(".json.claimed-") || name.ends_with(".tmp");
            let expired = name.strip_suffix(".json").is_some_and(valid_plan_id)
                && std::fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Plan<serde_json::Value>>(&b).ok())
                    .is_some_and(|p| now > p.expires_at_ms);
            if leftover || expired {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::TestDir;
    use super::super::gate::{Caller, authorize_write};
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Change {
        enable: String,
    }

    fn setup() -> (TestDir, PlanStore, PathBuf, Authorized) {
        let tmp = TestDir::new("plan");
        let store = PlanStore::new(tmp.0.join("plans"), 1);
        let file = tmp.0.join("config.json");
        std::fs::write(&file, "{}").unwrap();
        let cli = authorize_write(&Caller::Cli, false, None).unwrap();
        (tmp, store, file, cli)
    }

    fn plan_on(store: &PlanStore, file: &Path) -> Plan<Change> {
        store.save("target-a", Change { enable: "b".into() }, vec![Precondition::file(file).unwrap()]).unwrap()
    }

    #[test]
    fn a_plan_applies_once() {
        let (_tmp, store, file, cli) = setup();
        let plan = plan_on(&store, &file);
        assert!(valid_plan_id(&plan.plan_id));
        let taken: Plan<Change> = store.take(&cli, &plan.plan_id, "target-a", files_only, |_| Ok(())).unwrap();
        assert_eq!(taken.payload, Change { enable: "b".into() });
        let again = store.take::<Change>(&cli, &plan.plan_id, "target-a", files_only, |_| Ok(()));
        assert_eq!(again.unwrap_err().code, codes::PLAN_NOT_FOUND);
        assert_eq!(std::fs::read_dir(store.dir()).unwrap().count(), 0);
    }

    #[test]
    fn a_changed_file_or_record_refuses_and_keeps_the_plan() {
        let (_tmp, store, file, cli) = setup();
        let plan = plan_on(&store, &file);
        std::fs::write(&file, "{\"x\":1}").unwrap();
        let r = store.take::<Change>(&cli, &plan.plan_id, "target-a", files_only, |_| Ok(()));
        assert_eq!(r.unwrap_err().code, codes::CHANGED_MEANWHILE);
        assert!(store.path(&plan.plan_id).exists());

        let rec =
            store.save("t", Change { enable: "c".into() }, vec![Precondition::record("row:7", Some(b"v1"))]).unwrap();
        let same = |_: &str| Ok(Some(sha256_hex(b"v1")));
        let moved = |_: &str| Ok(Some(sha256_hex(b"v2")));
        assert_eq!(
            store.take::<Change>(&cli, &rec.plan_id, "t", moved, |_| Ok(())).unwrap_err().code,
            codes::CHANGED_MEANWHILE
        );
        assert_eq!(
            store.take::<Change>(&cli, &rec.plan_id, "t", files_only, |_| Ok(())).unwrap_err().code,
            codes::STORE_UNREADABLE
        );
        assert!(store.take::<Change>(&cli, &rec.plan_id, "t", same, |_| Ok(())).is_ok());
    }

    #[test]
    fn the_apps_own_recheck_can_refuse() {
        let (_tmp, store, file, cli) = setup();
        let plan = plan_on(&store, &file);
        let refuse = |_: &Plan<Change>| Err(AgentError::new(codes::CHANGED_MEANWHILE, "steps differ"));
        assert_eq!(
            store.take(&cli, &plan.plan_id, "target-a", files_only, refuse).unwrap_err().code,
            codes::CHANGED_MEANWHILE
        );
        assert!(store.path(&plan.plan_id).exists(), "a refused plan is not used up");
    }

    #[test]
    fn expired_edited_other_target_and_bad_ids_are_refused() {
        let (_tmp, store, file, cli) = setup();
        let plan = plan_on(&store, &file);
        let other = store.take::<Change>(&cli, &plan.plan_id, "target-b", files_only, |_| Ok(()));
        assert_eq!(other.unwrap_err().code, codes::PLAN_OTHER_TARGET);

        let late = plan.expires_at_ms + 1;
        let expired = store.take_at::<Change>(&cli, &plan.plan_id, "target-a", files_only, |_| Ok(()), late);
        assert_eq!(expired.unwrap_err().code, codes::PLAN_EXPIRED);
        assert!(!store.path(&plan.plan_id).exists(), "an expired plan is deleted");

        let edited = plan_on(&store, &file);
        let path = store.path(&edited.plan_id);
        let text = std::fs::read_to_string(&path).unwrap().replace("\"b\"", "\"z\"");
        std::fs::write(&path, text).unwrap();
        let r = store.take::<Change>(&cli, &edited.plan_id, "target-a", files_only, |_| Ok(()));
        assert_eq!(r.unwrap_err().code, codes::CHANGED_MEANWHILE);

        for id in ["../config", "0123456789ABCDEF", "", "0123456789abcdef0"] {
            assert_eq!(
                store.take::<Change>(&cli, id, "target-a", files_only, |_| Ok(())).unwrap_err().code,
                codes::PLAN_NOT_FOUND,
                "{id}"
            );
        }
    }

    #[test]
    fn the_content_hash_is_canonical_json() {
        let mut text = String::new();
        canonical(&serde_json::json!({ "b": [1, { "d": null, "c": "x" }], "a": true }), &mut text);
        assert_eq!(text, r#"{"a":true,"b":[1,{"c":"x","d":null}]}"#);
        // The same value in the TypeScript kit's test.
        let pre = vec![Precondition { kind: PreconditionKind::File, key: "/x".into(), sha256: None }];
        assert_eq!(
            content_hash("t", &serde_json::json!({ "enable": "b" }), &pre),
            sha256_hex(br#"{"payload":{"enable":"b"},"preconditions":[{"key":"/x","kind":"file","sha256":null}],"target":"t"}"#)
        );
    }

    #[test]
    fn saving_prunes_expired_plans_and_leftovers() {
        let (_tmp, store, file, _cli) = setup();
        let short = store.clone().with_lifetime_ms(5);
        let old = short.save_at("t", Change { enable: "x".into() }, vec![], 1_000).unwrap();
        std::fs::write(store.dir().join("0000000000000000.json.claimed-1"), "").unwrap();
        let _ = Precondition::file(&file).unwrap();
        let fresh = store.save_at("t", Change { enable: "y".into() }, vec![], 2_000).unwrap();
        let names: Vec<String> = std::fs::read_dir(store.dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [format!("{}.json", fresh.plan_id)]);
        assert_ne!(old.plan_id, fresh.plan_id);
        assert_eq!(Precondition::file(&store.dir().join("none")).unwrap().sha256, None);
    }
}
