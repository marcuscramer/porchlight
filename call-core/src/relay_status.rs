//! What the "Connection info" screens say about each relay, decided once for
//! both shells.
//!
//! The shells report what happened (a message arrived, a connection failed or
//! came up; a publish rejection is noted by `signal_retry` itself) and ask for
//! one ready-to-render row per relay. Before this each shell kept its own
//! three maps and its own rules for the dot colour, which error to show, how
//! to trim it and how to phrase "how long ago", and they had drifted.
//!
//! The last rejection per relay is kept across restarts together with the
//! cooldown it caused ([`export_memory`]/[`import_memory`]): a pause without
//! its reason is a mystery after a restart.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Longest error text shown (characters).
const MAX_ERROR_CHARS: usize = 200;
/// Saved rejections older than this are not worth showing after a restart.
const MAX_IMPORTED_REJECT_AGE_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Note {
    reason: String,
    at_ms: i64,
}

/// A field of `crate::AppState`, like the other modules' state.
pub(crate) struct RelayLogState {
    last_message_at: HashMap<String, i64>,
    /// Why a relay is not connected; cleared the moment it connects.
    connect_error: HashMap<String, Note>,
    /// Why it last rejected a publish; kept (it explains a pause).
    last_reject: HashMap<String, Note>,
}

impl RelayLogState {
    pub(crate) fn new() -> Self {
        RelayLogState { last_message_at: HashMap::new(), connect_error: HashMap::new(), last_reject: HashMap::new() }
    }

    /// Called by `signal_retry` for every rejected publish.
    pub(crate) fn note_reject(&mut self, relay: &str, reason: &str, now_ms: i64) {
        self.last_reject.insert(relay.to_string(), Note { reason: reason.to_string(), at_ms: now_ms });
    }
}

/// Relay URLs reach the shells with and without a trailing slash (the web
/// pool normalizes them); everything here is keyed without one.
fn key(relay: &str) -> String {
    relay.trim_end_matches('/').to_string()
}

pub fn note_message(relay: &str, now_ms: i64) {
    crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).relay_log.last_message_at.insert(key(relay), now_ms);
}

pub fn note_connected(relay: &str) {
    crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).relay_log.connect_error.remove(&key(relay));
}

/// `at_ms` is when the failure began (a shell that sees one failure as
/// several events keeps the first time).
pub fn note_connect_error(relay: &str, reason: &str, at_ms: i64) {
    crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).relay_log.connect_error.insert(key(relay), Note { reason: reason.to_string(), at_ms });
}

/// How long ago something happened, for display.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(tag = "unit", content = "n", rename_all = "snake_case")]
pub enum Ago {
    Never,
    Seconds(u64),
    Minutes(u64),
    Hours(u64),
}

pub fn ago(at_ms: Option<i64>, now_ms: i64) -> Ago {
    let Some(at) = at_ms else { return Ago::Never };
    let seconds = ((now_ms - at) / 1_000).max(0) as u64;
    match seconds {
        0..=59 => Ago::Seconds(seconds),
        60..=3_599 => Ago::Minutes(seconds / 60),
        _ => Ago::Hours(seconds / 3_600),
    }
}

/// What the dot says: green = connected and in use, yellow = connected but
/// paused after a rejection, red = not connected.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelayState {
    Connected,
    Paused,
    Down,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct RelayView {
    pub url: String,
    /// Shown name: the URL without scheme and trailing slash.
    pub host: String,
    pub state: RelayState,
    pub accepted: u32,
    pub rejected: u32,
    /// Why it is down or paused, trimmed for one line. `None` when there is
    /// nothing to explain.
    pub error: Option<String>,
    /// When `error` happened, or — without one — when the last message
    /// arrived.
    pub ago: Ago,
}

/// Short, single-line error text: a library's `Error:` prefix and its
/// trailing `. Exception: …` chain are noise.
fn trim_error(reason: &str) -> String {
    let reason = reason.trim();
    let reason = reason.strip_prefix("Error:").map(str::trim_start).unwrap_or(reason);
    let reason = reason.split(". Exception:").next().unwrap_or(reason);
    reason.chars().take(MAX_ERROR_CHARS).collect()
}

/// One row per entry of `relays`, in that order. `connected` are the relays
/// whose connection is up right now.
pub fn view(relays: &[String], connected: &[String], now_ms: i64) -> Vec<RelayView> {
    let app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let log = &app.relay_log;
    let up: Vec<String> = connected.iter().map(|c| key(c)).collect();
    relays
        .iter()
        .map(|url| {
            let k = key(url);
            let is_up = up.contains(&k);
            let paused = app.signal_retry.is_cooling_down(&k, now_ms) || app.signal_retry.is_cooling_down(url, now_ms);
            let state = if !is_up {
                RelayState::Down
            } else if paused {
                RelayState::Paused
            } else {
                RelayState::Connected
            };
            // Not connected: why. Connected but paused: the rejection behind
            // the pause. Otherwise nothing to explain.
            let note = match state {
                RelayState::Connected => None,
                RelayState::Paused => log.last_reject.get(&k),
                RelayState::Down => log.connect_error.get(&k).or_else(|| log.last_reject.get(&k)),
            };
            let stats = app.signal_retry.stats_for(&k).or_else(|| app.signal_retry.stats_for(url)).unwrap_or_default();
            RelayView {
                url: url.clone(),
                host: k.trim_start_matches("wss://").trim_start_matches("ws://").to_string(),
                state,
                accepted: stats.accepted,
                rejected: stats.rejected,
                error: note.map(|n| trim_error(&n.reason)),
                ago: ago(note.map(|n| n.at_ms).or_else(|| log.last_message_at.get(&k).copied()), now_ms),
            }
        })
        .collect()
}

#[derive(Serialize, Deserialize, Default)]
struct Memory {
    #[serde(default)]
    cooldowns: BTreeMap<String, i64>,
    #[serde(default)]
    rejects: BTreeMap<String, Note>,
}

/// The running cooldowns and each relay's last rejection, as one JSON string
/// for the shell to keep across restarts.
pub fn export_memory(now_ms: i64) -> String {
    let cooldowns: BTreeMap<String, i64> = serde_json::from_str(&crate::signal_retry::export_cooldowns(now_ms)).unwrap_or_default();
    let app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let rejects = app.relay_log.last_reject.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    serde_json::to_string(&Memory { cooldowns, rejects }).unwrap_or_else(|_| "{}".to_string())
}

/// Restores [`export_memory`]'s output. The earlier format — the cooldown map
/// alone — is still read. Malformed input is ignored.
pub fn import_memory(json: &str, now_ms: i64) {
    let memory: Memory = match serde_json::from_str::<serde_json::Value>(json) {
        Ok(v) if v.get("cooldowns").is_some() || v.get("rejects").is_some() => serde_json::from_value(v).unwrap_or_default(),
        Ok(v) => Memory { cooldowns: serde_json::from_value(v).unwrap_or_default(), rejects: BTreeMap::new() },
        Err(_) => return,
    };
    crate::signal_retry::import_cooldowns(&serde_json::to_string(&memory.cooldowns).unwrap_or_default(), now_ms);
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    for (relay, note) in memory.rejects {
        if now_ms - note.at_ms <= MAX_IMPORTED_REJECT_AGE_MS {
            app.relay_log.last_reject.entry(relay).or_insert(note);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal_retry;

    fn reset() -> std::sync::MutexGuard<'static, ()> {
        // signal_retry's own serialization lock: its tests share this state.
        let guard = signal_retry::tests::reset_state_for_test();
        crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).relay_log = RelayLogState::new();
        guard
    }

    fn urls(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ago_buckets() {
        assert_eq!(ago(None, 10_000), Ago::Never);
        assert_eq!(ago(Some(10_000), 10_000), Ago::Seconds(0));
        assert_eq!(ago(Some(0), 59_999), Ago::Seconds(59));
        assert_eq!(ago(Some(0), 60_000), Ago::Minutes(1));
        assert_eq!(ago(Some(0), 3_599_000), Ago::Minutes(59));
        assert_eq!(ago(Some(0), 3_600_000), Ago::Hours(1));
        // A clock that went backwards doesn't show a negative age.
        assert_eq!(ago(Some(5_000), 1_000), Ago::Seconds(0));
    }

    #[test]
    fn errors_are_trimmed_to_one_short_line() {
        assert_eq!(trim_error("Error: connection failed"), "connection failed");
        assert_eq!(trim_error("rate-limited: slow down. Exception: java.io.IOException: x"), "rate-limited: slow down");
        assert_eq!(trim_error(&"x".repeat(500)).chars().count(), MAX_ERROR_CHARS);
    }

    #[test]
    fn a_connected_relay_in_use_has_nothing_to_explain_and_shows_its_last_message() {
        let _g = reset();
        note_message("wss://a.example/", 5_000);
        let rows = view(&urls(&["wss://a.example"]), &urls(&["wss://a.example/"]), 8_000);
        assert_eq!(rows[0].state, RelayState::Connected);
        assert_eq!(rows[0].host, "a.example");
        assert_eq!(rows[0].error, None);
        assert_eq!(rows[0].ago, Ago::Seconds(3));
    }

    #[test]
    fn a_relay_that_is_down_says_why_and_when() {
        let _g = reset();
        note_message("wss://a.example", 1_000);
        note_connect_error("wss://a.example", "Error: connection failed (code 1006)", 4_000);
        let rows = view(&urls(&["wss://a.example"]), &[], 10_000);
        assert_eq!(rows[0].state, RelayState::Down);
        assert_eq!(rows[0].error.as_deref(), Some("connection failed (code 1006)"));
        assert_eq!(rows[0].ago, Ago::Seconds(6));
        // Connecting clears it.
        note_connected("wss://a.example");
        let rows = view(&urls(&["wss://a.example"]), &[], 10_000);
        assert_eq!(rows[0].error, None);
        assert_eq!(rows[0].ago, Ago::Seconds(9));
    }

    #[test]
    fn a_relay_that_rejected_a_publish_is_paused_with_the_reason_while_connected() {
        let _g = reset();
        signal_retry::record_publish_result("e1", "wss://a.example", false, "rate-limited: slow down", 1_000);
        let rows = view(&urls(&["wss://a.example"]), &urls(&["wss://a.example"]), 2_000);
        assert_eq!(rows[0].state, RelayState::Paused);
        assert_eq!(rows[0].error.as_deref(), Some("rate-limited: slow down"));
        assert_eq!(rows[0].rejected, 1);
        // Once the cooldown is over the reason is no longer shown.
        let later = view(&urls(&["wss://a.example"]), &urls(&["wss://a.example"]), 10 * 60 * 60 * 1000);
        assert_eq!(later[0].state, RelayState::Connected);
        assert_eq!(later[0].error, None);
    }

    #[test]
    fn a_down_relay_falls_back_to_its_last_rejection() {
        let _g = reset();
        signal_retry::record_publish_result("e1", "wss://a.example", false, "blocked: not on the list", 1_000);
        let rows = view(&urls(&["wss://a.example"]), &[], 2_000);
        assert_eq!(rows[0].state, RelayState::Down);
        assert_eq!(rows[0].error.as_deref(), Some("blocked: not on the list"));
    }

    #[test]
    fn counts_come_from_the_publish_results() {
        let _g = reset();
        signal_retry::record_publish_result("e1", "wss://a.example", true, "", 1_000);
        signal_retry::record_publish_result("e2", "wss://a.example", true, "", 1_000);
        let rows = view(&urls(&["wss://a.example"]), &urls(&["wss://a.example"]), 2_000);
        assert_eq!((rows[0].accepted, rows[0].rejected), (2, 0));
    }

    #[test]
    fn memory_round_trips_and_reads_the_old_cooldown_only_format() {
        let _g = reset();
        signal_retry::record_publish_result("e1", "wss://a.example", false, "rate-limited: slow down", 1_000);
        let saved = export_memory(2_000);
        {
            let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
            app.relay_log = RelayLogState::new();
            app.signal_retry = signal_retry::SignalRetryState::new();
        }
        import_memory(&saved, 3_000);
        let rows = view(&urls(&["wss://a.example"]), &urls(&["wss://a.example"]), 4_000);
        assert_eq!(rows[0].state, RelayState::Paused);
        assert_eq!(rows[0].error.as_deref(), Some("rate-limited: slow down"));

        // The earlier format: just url -> cooldown end.
        {
            let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
            app.relay_log = RelayLogState::new();
            app.signal_retry = signal_retry::SignalRetryState::new();
        }
        import_memory(r#"{"wss://a.example": 600000}"#, 3_000);
        let rows = view(&urls(&["wss://a.example"]), &urls(&["wss://a.example"]), 4_000);
        assert_eq!(rows[0].state, RelayState::Paused);
        assert_eq!(rows[0].error, None);
        // Garbage changes nothing.
        import_memory("not json", 3_000);
    }

    #[test]
    fn serializes_for_the_shells() {
        assert_eq!(serde_json::to_string(&Ago::Minutes(2)).unwrap(), r#"{"unit":"minutes","n":2}"#);
        assert_eq!(serde_json::to_string(&Ago::Never).unwrap(), r#"{"unit":"never"}"#);
        assert_eq!(serde_json::to_string(&RelayState::Paused).unwrap(), r#""paused""#);
    }
}
