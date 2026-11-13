//! The keeper's triggers and loop. After the run at sign-in, the keeper stays
//! running and puts kept devices back whenever something may have undone
//! them: the PC waking from sleep (reported by the front end, since that is a
//! Windows notice), OpenRGB saying its device list changed, or OpenRGB coming
//! back after a restart. RAM that loses power during sleep falls back to its
//! own effect without OpenRGB noticing, so every trigger re-sends the kept
//! setup rather than trusting OpenRGB's record.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::keep::{self, ActionError, Outcome, Run};
use crate::sdk::wire::packet;
use crate::sdk::{Client, SdkError};

/// Why the keeper is putting devices back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Start,
    Wake,
    DevicesChanged,
    OpenRgbBack,
    /// You asked, from the tray icon's menu.
    Asked,
}

impl Trigger {
    /// The end of a log sentence: "Put back 2 of 2 kept devices {reason}."
    pub fn reason(self) -> &'static str {
        match self {
            Trigger::Start => "when the keeper started",
            Trigger::Wake => "after the PC woke",
            Trigger::DevicesChanged => "after OpenRGB's device list changed",
            Trigger::OpenRgbBack => "after OpenRGB came back",
            Trigger::Asked => "when you asked",
        }
    }

    /// When several triggers arrive together, the run is named after the most
    /// telling one: waking explains a device-list change, not the other way.
    fn rank(self) -> u8 {
        match self {
            Trigger::Asked => 4,
            Trigger::Start => 3,
            Trigger::Wake => 2,
            Trigger::OpenRgbBack => 1,
            Trigger::DevicesChanged => 0,
        }
    }
}

/// How the watcher and loop pace themselves. Tests shorten these.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Quiet time to wait after a trigger, so a burst becomes one run.
    pub settle: Duration,
    /// How long one run waits for OpenRGB to list every kept device.
    pub wait: Duration,
    /// How often a run, or the watcher, tries OpenRGB again.
    pub poll: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing { settle: Duration::from_secs(3), wait: Duration::from_secs(60), poll: Duration::from_secs(3) }
    }
}

/// Listens to OpenRGB at `address` until `stop` is set: sends
/// `DevicesChanged` for each `DEVICE_LIST_UPDATED`, and `OpenRgbBack` when a
/// connection is made again after one was lost or refused. Sends no request
/// that changes a light.
pub fn watch(address: &str, triggers: &Sender<Trigger>, stop: &AtomicBool, timing: Timing) {
    let mut lost = false;
    while !stop.load(Ordering::Relaxed) {
        let mut client = match Client::connect(address) {
            Ok(c) => c,
            Err(_) => {
                lost = true;
                std::thread::sleep(timing.poll);
                continue;
            }
        };
        if lost && triggers.send(Trigger::OpenRgbBack).is_err() {
            return;
        }
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match client.next_notice(timing.poll) {
                Ok(Some(packet::DEVICE_LIST_UPDATED)) => {
                    if triggers.send(Trigger::DevicesChanged).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(SdkError::Io(_)) | Err(SdkError::Unreachable { .. }) | Err(SdkError::Protocol(_)) => {
                    lost = true;
                    break;
                }
            }
        }
    }
}

/// Runs [`keep::put_back_all`] for each trigger until every sender is gone,
/// calling `report` after each run. Triggers that arrive within
/// `timing.settle` of each other become one run.
pub fn run(
    address: &str,
    setup_path: &Path,
    triggers: &Receiver<Trigger>,
    timing: Timing,
    mut report: impl FnMut(Trigger, &Result<Run, ActionError>),
) {
    while let Ok(first) = triggers.recv() {
        let mut why = first;
        let mut last = false;
        loop {
            match triggers.recv_timeout(timing.settle) {
                Ok(t) if t.rank() > why.rank() => why = t,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    last = true;
                    break;
                }
            }
        }
        let result = keep::put_back_all(address, setup_path, timing.wait, timing.poll);
        report(why, &result);
        if last {
            return;
        }
    }
}

/// What went wrong in a run, one sentence per problem; empty when every kept
/// device held.
pub fn problems(result: &Result<Run, ActionError>) -> Vec<String> {
    let run = match result {
        Ok(run) => run,
        Err(e) => return vec![format!("Couldn't put any kept device back. {e}")],
    };
    run.reports
        .iter()
        .filter_map(|r| match &r.result {
            Ok(Outcome::Held) => None,
            Ok(Outcome::NotHeld) => Some(format!("{} was sent its color but reads back as something else.", r.label())),
            Ok(Outcome::Skipped(reason)) | Err(reason) => Some(format!("{} wasn't put back: {reason}.", r.label())),
        })
        .collect()
}

/// Decides when to tell you about problems: once when they appear or change,
/// not again on every wake while they stay the same, and again after they
/// clear and come back.
#[derive(Debug, Default)]
pub struct Notices {
    last: Vec<String>,
}

/// What to show in a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

impl Notices {
    pub fn next(&mut self, problems: Vec<String>) -> Option<Notice> {
        if problems == self.last {
            return None;
        }
        self.last = problems;
        let first = self.last.first()?;
        let more = self.last.len() - 1;
        let body = if more == 0 { first.clone() } else { format!("{first} And {more} more; the log has them all.") };
        Some(Notice { title: "Keeplight couldn't put a light back".into(), body })
    }
}

/// A flag the front end sets to stop [`watch`].
pub fn stop_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    use super::*;
    use crate::keep::{Kept, Setup};
    use crate::testing::{self, FakeServer};

    const QUICK: Timing =
        Timing { settle: Duration::from_millis(100), wait: Duration::from_secs(2), poll: Duration::from_millis(20) };

    fn wait_for(what: &str, test: impl Fn() -> bool) {
        let start = Instant::now();
        while !test() {
            assert!(start.elapsed() < Duration::from_secs(5), "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_device_list_change_becomes_a_trigger() {
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        let (tx, rx) = mpsc::channel();
        let stop = stop_flag();
        let (address, flag) = (server.address.clone(), stop.clone());
        let watcher = thread::spawn(move || watch(&address, &tx, &flag, QUICK));
        wait_for("the watcher to connect", || server.client_count() == 1);
        server.announce_device_change();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(Trigger::DevicesChanged));
        stop.store(true, Ordering::Relaxed);
        watcher.join().unwrap();
        let ids = server.seen_ids();
        assert!(!ids.contains(&packet::UPDATE_MODE) && !ids.contains(&packet::UPDATE_LEDS), "the watcher only listens");
    }

    #[test]
    fn openrgb_coming_back_becomes_a_trigger() {
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        let (tx, rx) = mpsc::channel();
        let stop = stop_flag();
        let (address, flag) = (server.address.clone(), stop.clone());
        let watcher = thread::spawn(move || watch(&address, &tx, &flag, QUICK));
        wait_for("the watcher to connect", || server.client_count() == 1);
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err(), "the first connection is not a trigger");
        server.drop_clients();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(Trigger::OpenRgbBack));
        stop.store(true, Ordering::Relaxed);
        watcher.join().unwrap();
    }

    #[test]
    fn a_burst_of_triggers_is_one_run_named_after_the_best_reason() {
        let path = std::env::temp_dir().join(format!("keeplight-test-{}-burst/setup.json", std::process::id()));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let mut setup = Setup::default();
        setup.keep(Kept::capture(&testing::ram("SMBus 0x71")).unwrap());
        setup.save(&path).unwrap();
        let mut rainbow = testing::ram("SMBus 0x71");
        rainbow.active_mode = Some(1);
        let server = FakeServer::start(vec![rainbow]);

        let (tx, rx) = mpsc::channel();
        tx.send(Trigger::DevicesChanged).unwrap();
        tx.send(Trigger::Wake).unwrap();
        tx.send(Trigger::DevicesChanged).unwrap();
        drop(tx);
        let mut runs = Vec::new();
        run(&server.address, &path, &rx, QUICK, |why, result| {
            runs.push((why, result.as_ref().map(|r| r.reports[0].result.clone()).unwrap()))
        });
        assert_eq!(runs, [(Trigger::Wake, Ok(Outcome::Held))]);
        assert_eq!(server.device(0).active_mode_name(), Some("Chase Fade"));
    }

    fn report(location: &str, result: Result<Outcome, String>) -> crate::keep::Report {
        crate::keep::Report { name: "ENE DRAM".into(), location: location.into(), result }
    }

    #[test]
    fn problems_are_one_sentence_each_and_none_when_everything_held() {
        let ok = Run { reports: vec![report("SMBus 0x71", Ok(Outcome::Held))], waited: Duration::ZERO };
        assert!(problems(&Ok(ok)).is_empty());
        let bad = Run {
            reports: vec![
                report("I2C: PawnIO SMBus i801 0, address 0x71", Ok(Outcome::Held)),
                report("I2C: PawnIO SMBus i801 0, address 0x73", Err("OpenRGB doesn't list it".into())),
                report("SMBus 0x75", Ok(Outcome::NotHeld)),
            ],
            waited: Duration::ZERO,
        };
        assert_eq!(
            problems(&Ok(bad)),
            [
                "ENE DRAM, address 0x73 wasn't put back: OpenRGB doesn't list it.",
                "ENE DRAM at SMBus 0x75 was sent its color but reads back as something else.",
            ]
        );
        assert_eq!(problems(&Err(ActionError::NotFound)).len(), 1);
    }

    #[test]
    fn the_same_problems_are_told_once() {
        let mut notices = Notices::default();
        let gone = vec!["ENE DRAM, address 0x73 wasn't put back: OpenRGB doesn't list it.".to_string()];
        let n = notices.next(gone.clone()).unwrap();
        assert_eq!(n.body, gone[0]);
        assert_eq!(notices.next(gone.clone()), None, "not again on the next wake");
        assert_eq!(notices.next(vec![]), None, "clearing tells nothing");
        assert!(notices.next(gone.clone()).is_some(), "but coming back is told again");
        let two = vec![gone[0].clone(), "Another.".to_string()];
        assert!(notices.next(two).unwrap().body.ends_with("And 1 more; the log has them all."));
    }

    #[test]
    fn every_trigger_resends_even_when_openrgb_thinks_nothing_changed() {
        // After sleep, OpenRGB still records the RAM as kept while the stick
        // shows its own rainbow, so a run must send anyway.
        let path = std::env::temp_dir().join(format!("keeplight-test-{}-resend/setup.json", std::process::id()));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let mut setup = Setup::default();
        setup.keep(Kept::capture(&testing::ram("SMBus 0x71")).unwrap());
        setup.save(&path).unwrap();
        let server = FakeServer::start(vec![testing::ram("SMBus 0x71")]);
        let (tx, rx) = mpsc::channel();
        tx.send(Trigger::Wake).unwrap();
        drop(tx);
        run(&server.address, &path, &rx, QUICK, |_, _| {});
        assert!(server.seen_ids().contains(&packet::UPDATE_MODE));
    }
}
