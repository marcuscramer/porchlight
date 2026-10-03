//! Pure bookkeeping for the signaling-publish retry queue — `pendingPublishes`/
//! `retryPendingPublishes` in the old independently hand-written `app.js`
//! (`web-app/app.js`), and their newer `NostrSignalingClient.kt` twin. Exists
//! because neither platform's relay-client library retries a lost publish on
//! its own: nostr-tools' `SimplePool.publish()` is a single attempt per relay
//! with no resend when a relay that was briefly down comes back, and
//! quartz-android's `NostrClient` has the identical gap — it tracks per-relay
//! send state in an internal `PoolEventOutbox`, but nothing in that library
//! ever reads it back out to retry anything (confirmed by decompiling the
//! library itself, both the version this app pins and the current one).
//! Without this, every signal message — offer/answer/ice/call/busy/bye and
//! the presence heartbeat itself — is one shot: lost to a transient relay
//! hiccup, lost for good.
//!
//! **What this module doesn't own**: the actual publish call, and which
//! relays are live right now — the same category [`crate::nostr_protocol`]'s
//! own doc already draws around the live relay connection itself. A shell
//! calls [`record_pending_publish`] with whichever of its own relay targets
//! it already knows weren't connected at send time, translates its own
//! platform's real per-relay delivery signal (a genuine NIP-01 `OK` message
//! on Android, a settled `publish()` promise on web — neither is "the local
//! socket accepted the write," which carries no information about whether
//! the relay actually got it) into [`record_publish_result`], and republishes
//! whatever [`due_for_retry`] hands back on its own existing periodic tick.
//! `relays` cross as plain `String`s, not a richer type — each shell has its
//! own relay-client library's own URL type and translates this plain data
//! into it, same reasoning as [`crate::nostr_protocol::FilterSpec`].
//!
//! A relay that explicitly rejects a publish also gets a cooldown, owned
//! here too ([`record_publish_result`]/[`available_relays`]/[`due_for_retry`]):
//! the old `app.js` had one (after a relay demanded proof-of-work and the
//! queue retried that doomed publish every 10s forever) while the Android
//! side had nothing, which is exactly the drift moving this into the shared
//! crate prevents.
//!
//! A relay that already has the event just gets a harmless redelivery on
//! retry — the receiving side's own [`crate::nostr_protocol::mark_seen_or_is_duplicate`]
//! guards against acting on it twice, regardless of how many times it
//! arrives.

use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

/// How long after being sent a message is still worth retrying, by what it
/// says (`None`: never queue it) — decided here, from the plain payload the
/// shell already has, so both platforms share one policy:
///
/// - **Call signaling** (`call`/`offer`/`answer`/`ice`/`busy`/`bye`) matters
///   exactly as long as the call attempt it belongs to can still succeed —
///   [`crate::call_arbitration::CALL_ANSWER_TIMEOUT_MS`], after which the
///   caller has given up on its own. Retrying longer would ring a peer's
///   phone for a call the caller already abandoned.
/// - **Heartbeats** are never queued: the next one is at most one heartbeat
///   interval away and supersedes it, and a stale one arriving late would
///   claim the sender is still online (and still busy, or not) when it may
///   have left in between.
/// - **Pairing bootstrap** messages are already republished on every
///   heartbeat tick by the shell, which is a better retry than this one.
/// - **`leaving`** is only meaningful in the instant the device goes away.
fn retry_ttl_ms(payload_json: &str) -> Option<i64> {
    let value: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    match value.get("type")?.as_str()? {
        "call" | "offer" | "answer" | "ice" | "busy" | "bye" => Some(crate::call_arbitration::CALL_ANSWER_TIMEOUT_MS),
        _ => None,
    }
}

/// Hard cap so a long offline stretch can't grow the queue unboundedly.
/// Matches `app.js`'s own `PENDING_PUBLISH_MAX_ENTRIES` exactly.
const PENDING_PUBLISH_MAX_ENTRIES: usize = 50;

/// How long a relay that explicitly rejected a publish is left alone —
/// retrying the identical event on the very next tick essentially never
/// succeeds before whatever policy triggered the rejection changes on its
/// own, and for a rate-limit-shaped reason it risks *extending* the
/// throttle. Every explicit rejection earns a cooldown, not just ones
/// matching a known reason string (found live 2026-10-01 when `nos.lol`
/// rejected every publish with "pow: 28 bits needed" and an earlier queue,
/// which only cooled down reasons it recognized, retried that doomed
/// publish every 10s forever): a relay can phrase a rejection however it
/// wants, so [`looks_likely_permanent`] only decides *how long*.
const REJECTION_COOLDOWN_MS: i64 = 2 * 60 * 1000;

/// Longer cooldown for a rejection whose reason reads like a rate limit, ban,
/// or proof-of-work demand — extremely unlikely to resolve soon.
const PERMANENT_REJECTION_COOLDOWN_MS: i64 = 10 * 60 * 1000;

/// One publish still outstanding at one or more relays. `relays` narrows
/// down in place as results arrive via [`record_publish_result`] — the entry is
/// dropped the moment it empties, in that function, not left for
/// [`due_for_retry`] to notice.
struct PendingPublish {
    event_id: String,
    event_json: String,
    relays: HashSet<String>,
    expires_at_ms: i64,
}

/// A field of `crate::AppState`, not its own separately-locked static —
/// same reasoning as [`crate::presence::PresenceState`]'s own doc.
/// `VecDeque`, not a `Vec`: insertion order matters for
/// [`record_pending_publish`]'s evict-oldest bound, same shape as
/// [`crate::nostr_protocol::DedupState`]'s own `order` field.
pub(crate) struct SignalRetryState {
    pending: VecDeque<PendingPublish>,
    /// Relay URL -> when its rejection cooldown ends. At most one entry per
    /// configured relay, so never pruned.
    cooldown_until_ms: HashMap<String, i64>,
}

impl SignalRetryState {
    pub(crate) fn new() -> Self {
        SignalRetryState { pending: VecDeque::new(), cooldown_until_ms: HashMap::new() }
    }

    fn is_cooling_down(&self, relay: &str, now_ms: i64) -> bool {
        self.cooldown_until_ms.get(relay).is_some_and(|&until| until > now_ms)
    }
}

/// Mirrors the old `app.js` `/rate.?limit|banned|too many|pow:|proof.?of.?work|difficulty/i`
/// exactly, without a regex dependency: `.?` is "optionally one character".
fn looks_likely_permanent(reason: &str) -> bool {
    let reason: Vec<char> = reason.to_lowercase().chars().collect();
    let contains = |needle: &str| {
        let n: Vec<char> = needle.chars().collect();
        reason.windows(n.len()).any(|w| w == n.as_slice())
    };
    contains("banned") || contains("too many") || contains("pow:") || contains("difficulty") || matches_gapped(&reason, &["rate", "limit"]) || matches_gapped(&reason, &["proof", "of", "work"])
}

/// Whether `segs` appear in order in `hay`, each pair separated by zero or
/// one arbitrary character.
fn matches_gapped(hay: &[char], segs: &[&str]) -> bool {
    let first: Vec<char> = segs[0].chars().collect();
    (0..hay.len()).any(|start| hay[start..].starts_with(&first) && rest_matches(&hay[start + first.len()..], &segs[1..]))
}

fn rest_matches(hay: &[char], segs: &[&str]) -> bool {
    let Some(seg) = segs.first() else { return true };
    let seg: Vec<char> = seg.chars().collect();
    [0usize, 1].iter().any(|&gap| hay.len() >= gap && hay[gap..].starts_with(&seg) && rest_matches(&hay[gap + seg.len()..], &segs[1..]))
}

/// What [`due_for_retry`] hands back for the shell to actually republish —
/// `event_json` is whatever the shell itself originally built and signed
/// (this module never constructs or inspects an event, purely strings in,
/// strings back out, same as the rest of this crate's FFI shape), `relays`
/// is this entry's still-outstanding targets.
#[derive(Serialize, Debug, PartialEq)]
pub struct PendingRetry {
    pub event_json: String,
    pub relays: Vec<String>,
}

/// Records `event_json` (already built/signed by the shell; this module
/// never looks inside it) as needing delivery to `relays` — unless
/// `payload_json`, the plain message it wraps, is one that shouldn't be
/// retried at all (see [`retry_ttl_ms`]; a no-op then) — call right
/// after the shell's own publish attempt, passing only whichever targets
/// it already knows missed (e.g. weren't connected at send time; a relay
/// that *was* sent to but still rejects it is narrowed back in separately,
/// via [`record_publish_result`]). A no-op for an empty `relays`. Evicts the
/// single oldest entry once the queue exceeds [`PENDING_PUBLISH_MAX_ENTRIES`],
/// same shape as [`crate::nostr_protocol::mark_seen_or_is_duplicate`]'s own
/// bound.
pub fn record_pending_publish(event_id: &str, event_json: &str, payload_json: &str, relays: &[String], now_ms: i64) {
    let Some(ttl_ms) = retry_ttl_ms(payload_json) else { return };
    if relays.is_empty() {
        return;
    }
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.signal_retry;
    if state.pending.len() >= PENDING_PUBLISH_MAX_ENTRIES {
        state.pending.pop_front();
    }
    state.pending.push_back(PendingPublish {
        event_id: event_id.to_string(),
        event_json: event_json.to_string(),
        relays: relays.iter().cloned().collect(),
        expires_at_ms: now_ms + ttl_ms,
    });
}

/// One relay's real outcome for `event_id` — what the shell's own delivery
/// signal translates into. `accepted` (or a `duplicate:` rejection, which
/// means the relay already has the event) drops `relay` from the entry's
/// still-outstanding set, dropping the whole entry once empty. Anything else
/// is an explicit rejection: `relay` goes (back) into the entry — a no-op if
/// it was already there, or if `event_id` isn't pending at all — and starts
/// a cooldown (see [`REJECTION_COOLDOWN_MS`]) that keeps [`due_for_retry`]
/// and [`available_relays`] away from it. `reason` is the relay's own text,
/// or whatever the shell's library surfaces for a rejection; empty is fine.
pub fn record_publish_result(event_id: &str, relay: &str, accepted: bool, reason: &str, now_ms: i64) {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.signal_retry;
    let resolved = accepted || reason.to_lowercase().contains("duplicate:");
    if !resolved {
        let cooldown = if looks_likely_permanent(reason) { PERMANENT_REJECTION_COOLDOWN_MS } else { REJECTION_COOLDOWN_MS };
        state.cooldown_until_ms.insert(relay.to_string(), now_ms + cooldown);
    }
    let Some(entry) = state.pending.iter_mut().find(|p| p.event_id == event_id) else { return };
    if resolved {
        entry.relays.remove(relay);
        if entry.relays.is_empty() {
            state.pending.retain(|p| p.event_id != event_id);
        }
    } else {
        entry.relays.insert(relay.to_string());
    }
}

/// Which of `candidates` are not currently cooling down from a rejection —
/// what a shell should publish to on its *first* attempt, so a relay that
/// just rejected something isn't hammered by every new message either. The
/// ones left out belong in [`record_pending_publish`] like any other miss.
pub fn available_relays(candidates: &[String], now_ms: i64) -> Vec<String> {
    let app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    candidates.iter().filter(|r| !app.signal_retry.is_cooling_down(r, now_ms)).cloned().collect()
}

/// Called from the shell's own existing periodic tick (the same one
/// `check_online_timeouts`/`check_call_timeout` already run on) — prunes
/// anything past its own lifetime ([`retry_ttl_ms`]) first, then hands back every
/// remaining entry's still-outstanding relays that aren't cooling down from a
/// rejection, for the shell to actually republish to. An entry whose relays
/// are all cooling down this tick is simply left out (it stays queued).
/// Resending to a relay that already has the event is harmless (see this
/// module's own doc).
pub fn due_for_retry(now_ms: i64) -> Vec<PendingRetry> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.signal_retry;
    state.pending.retain(|p| now_ms < p.expires_at_ms);
    state
        .pending
        .iter()
        .filter_map(|p| {
            let relays: Vec<String> = p.relays.iter().filter(|r| !state.is_cooling_down(r, now_ms)).cloned().collect();
            (!relays.is_empty()).then(|| PendingRetry { event_json: p.event_json.clone(), relays })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes this module's own tests against each other — `cargo test`
    /// runs tests on separate threads by default, and every test here
    /// touches the same shared `crate::STATE.signal_retry`, so without this
    /// two tests running concurrently could interleave. Same shape as
    /// [`crate::call_arbitration::TEST_SERIAL`].
    static TEST_SERIAL: Mutex<()> = Mutex::new(());

    /// Resets this module's own field of the shared `crate::STATE` to a
    /// clean slate and returns the serialization guard above — held by the
    /// caller for its test's whole duration (`let _guard = ...`), same
    /// pattern as [`crate::presence::tests::reset_state_for_test`].
    fn reset_state_for_test() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).signal_retry = SignalRetryState::new();
        guard
    }

    const CALL: &str = r#"{"type":"offer","sdp":"x","callId":"c"}"#;

    /// An entry's still-outstanding relays regardless of cooldown (what
    /// `due_for_retry` deliberately hides), sorted; `None` if not queued.
    fn pending_relays(event_id: &str) -> Option<Vec<String>> {
        let app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
        let entry = app.signal_retry.pending.iter().find(|p| p.event_id == event_id)?;
        let mut v: Vec<String> = entry.relays.iter().cloned().collect();
        v.sort();
        Some(v)
    }

    fn relays(urls: &[&str]) -> Vec<String> {
        urls.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_fresh_pending_publish_is_due_for_retry_immediately() {
        let _guard = reset_state_for_test();
        record_pending_publish("id1", "{\"id\":\"id1\"}", CALL, &relays(&["wss://a", "wss://b"]), 1_000);
        let due = due_for_retry(1_000);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].event_json, "{\"id\":\"id1\"}");
        let mut got = due[0].relays.clone();
        got.sort();
        assert_eq!(got, vec!["wss://a".to_string(), "wss://b".to_string()]);
    }

    #[test]
    fn recording_an_empty_relay_list_is_a_no_op() {
        let _guard = reset_state_for_test();
        record_pending_publish("id1", "{}", CALL, &[], 1_000);
        assert!(due_for_retry(1_000).is_empty());
    }

    #[test]
    fn a_successful_ack_narrows_the_relay_set_and_drops_the_entry_once_empty() {
        let _guard = reset_state_for_test();
        record_pending_publish("id1", "{}", CALL, &relays(&["wss://a", "wss://b"]), 1_000);
        record_publish_result("id1", "wss://a", true, "", 1_001);
        let due = due_for_retry(1_001);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].relays, vec!["wss://b".to_string()]);

        record_publish_result("id1", "wss://b", true, "", 1_002);
        assert!(due_for_retry(1_002).is_empty(), "entry must be gone once every relay acked");
    }

    #[test]
    fn a_failed_result_adds_the_relay_back_in_even_if_it_wasnt_originally_missing() {
        let _guard = reset_state_for_test();
        // Only "wss://a" was missing at send time -- "wss://b" was sent to
        // but (per a later result) rejected it.
        record_pending_publish("id1", "{}", CALL, &relays(&["wss://a"]), 1_000);
        record_publish_result("id1", "wss://b", false, "blocked: nope", 1_001);
        // b is cooling down right now, so only a is due...
        assert_eq!(due_for_retry(1_001)[0].relays, vec!["wss://a".to_string()]);
        // ...but b really was added back to the entry.
        assert_eq!(pending_relays("id1"), Some(vec!["wss://a".to_string(), "wss://b".to_string()]));
    }

    #[test]
    fn an_ack_for_an_unknown_event_id_is_a_no_op() {
        let _guard = reset_state_for_test();
        record_publish_result("never-queued", "wss://a", true, "", 1_000);
        record_publish_result("never-queued", "wss://a", false, "", 1_000);
        assert!(due_for_retry(1_000).is_empty());
    }

    #[test]
    fn an_entry_past_max_age_is_dropped_silently() {
        let _guard = reset_state_for_test();
        record_pending_publish("id1", "{}", CALL, &relays(&["wss://a"]), 1_000);
        let due = due_for_retry(1_000 + crate::call_arbitration::CALL_ANSWER_TIMEOUT_MS - 1);
        assert_eq!(due.len(), 1, "not stale yet");
        let due = due_for_retry(1_000 + crate::call_arbitration::CALL_ANSWER_TIMEOUT_MS);
        assert!(due.is_empty(), "stale now -- strict less-than");
    }

    #[test]
    fn the_queue_evicts_the_oldest_entry_once_full() {
        let _guard = reset_state_for_test();
        for i in 0..PENDING_PUBLISH_MAX_ENTRIES {
            record_pending_publish(&format!("id{i}"), "{}", CALL, &relays(&["wss://a"]), 1_000);
        }
        assert_eq!(due_for_retry(1_000).len(), PENDING_PUBLISH_MAX_ENTRIES);

        // One more push evicts "id0", the oldest -- queue size stays capped,
        // and the evicted entry is actually gone, not just uncounted.
        record_pending_publish("idNew", "{}", CALL, &relays(&["wss://a"]), 1_000);
        let due = due_for_retry(1_000);
        assert_eq!(due.len(), PENDING_PUBLISH_MAX_ENTRIES);
        record_publish_result("id0", "wss://a", true, "", 1_000);
        assert_eq!(due_for_retry(1_000).len(), PENDING_PUBLISH_MAX_ENTRIES, "id0 was already gone, not emptied just now");
    }

    #[test]
    fn a_rejection_cools_the_relay_down_for_retries_and_first_publishes_then_expires() {
        let _guard = reset_state_for_test();
        record_pending_publish("id1", "{}", CALL, &relays(&["wss://a", "wss://b"]), 1_000);
        record_publish_result("id1", "wss://a", false, "blocked: spam", 1_000);

        let due = due_for_retry(1_001);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].relays, vec!["wss://b".to_string()], "the rejecting relay is skipped while cooling down");
        assert_eq!(available_relays(&relays(&["wss://a", "wss://b"]), 1_001), vec!["wss://b".to_string()]);

        // Still pending, just cooling down; and the cooldown really ends.
        assert_eq!(pending_relays("id1"), Some(vec!["wss://a".to_string(), "wss://b".to_string()]));
        let after = 1_000 + REJECTION_COOLDOWN_MS;
        assert_eq!(available_relays(&relays(&["wss://a"]), after), vec!["wss://a".to_string()]);
    }

    #[test]
    fn a_rejection_for_an_unqueued_event_still_cools_the_relay_down() {
        let _guard = reset_state_for_test();
        record_publish_result("never-queued", "wss://a", false, "", 1_000);
        assert!(available_relays(&relays(&["wss://a"]), 1_001).is_empty());
    }

    #[test]
    fn an_entry_whose_relays_are_all_cooling_down_is_left_out_but_stays_queued() {
        let _guard = reset_state_for_test();
        record_pending_publish("id1", "{}", CALL, &relays(&["wss://a"]), 1_000);
        record_publish_result("id1", "wss://a", false, "", 1_000);
        assert!(due_for_retry(1_001).is_empty());
        assert_eq!(pending_relays("id1"), Some(vec!["wss://a".to_string()]), "still queued, just not due");
    }

    #[test]
    fn a_permanent_looking_reason_gets_the_longer_cooldown() {
        let _guard = reset_state_for_test();
        record_publish_result("x", "wss://a", false, "pow: 28 bits needed", 1_000);
        record_publish_result("x", "wss://b", false, "blocked", 1_000);
        let just_after_short = 1_000 + REJECTION_COOLDOWN_MS;
        assert_eq!(available_relays(&relays(&["wss://a", "wss://b"]), just_after_short), vec!["wss://b".to_string()]);
        assert_eq!(available_relays(&relays(&["wss://a"]), 1_000 + PERMANENT_REJECTION_COOLDOWN_MS).len(), 1);
    }

    #[test]
    fn a_duplicate_rejection_counts_as_delivered_and_does_not_cool_the_relay_down() {
        let _guard = reset_state_for_test();
        record_pending_publish("id1", "{}", CALL, &relays(&["wss://a"]), 1_000);
        record_publish_result("id1", "wss://a", false, "duplicate: already have this event", 1_001);
        assert!(due_for_retry(1_002).is_empty(), "entry resolved");
        assert_eq!(available_relays(&relays(&["wss://a"]), 1_002).len(), 1);
    }

    #[test]
    fn looks_likely_permanent_matches_the_old_regex() {
        for yes in ["Rate limit exceeded", "rate-limited", "RATELIMIT", "you are banned", "too many requests", "pow: 28 bits needed", "proof of work required", "proof-of-work", "difficulty too low"] {
            assert!(looks_likely_permanent(yes), "{yes}");
        }
        for no in ["", "blocked: spam", "restricted: not on the allow list", "rate", "proof work", "unrelated"] {
            assert!(!looks_likely_permanent(no), "{no}");
        }
    }

    #[test]
    fn only_call_signaling_is_queued_for_retry() {
        let _guard = reset_state_for_test();
        for (n, payload) in [
            ("hb", r#"{"type":"heartbeat","name":"A","busy":false}"#),
            ("pake", r#"{"type":"pake1","pake":"00"}"#),
            ("leave", r#"{"type":"leaving"}"#),
            ("junk", "not json"),
            ("untyped", "{}"),
        ] {
            record_pending_publish(n, "{}", payload, &relays(&["wss://a"]), 1_000);
        }
        assert!(due_for_retry(1_000).is_empty(), "none of those are worth retrying");

        for (n, payload) in [
            ("call", r#"{"type":"call","callId":"c"}"#),
            ("offer", r#"{"type":"offer","sdp":"s","callId":"c"}"#),
            ("answer", r#"{"type":"answer","sdp":"s","callId":"c"}"#),
            ("ice", r#"{"type":"ice","candidate":"c","sdpMLineIndex":0,"callId":"c"}"#),
            ("busy", r#"{"type":"busy","callId":"c"}"#),
            ("bye", r#"{"type":"bye","callId":"c"}"#),
        ] {
            record_pending_publish(n, "{}", payload, &relays(&["wss://a"]), 1_000);
        }
        assert_eq!(due_for_retry(1_000).len(), 6);
    }
}
