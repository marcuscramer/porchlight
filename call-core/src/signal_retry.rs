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
//! the relay actually got it) into [`record_publish_ack`], and republishes
//! whatever [`due_for_retry`] hands back on its own existing periodic tick.
//! `relays` cross as plain `String`s, not a richer type — each shell has its
//! own relay-client library's own URL type and translates this plain data
//! into it, same reasoning as [`crate::nostr_protocol::FilterSpec`].
//!
//! A relay that already has the event just gets a harmless redelivery on
//! retry — the receiving side's own [`crate::nostr_protocol::mark_seen_or_is_duplicate`]
//! guards against acting on it twice, regardless of how many times it
//! arrives.

use serde::Serialize;
use std::collections::{HashSet, VecDeque};

/// Beyond this age, whatever a pending publish was about has almost
/// certainly moved on — [`due_for_retry`] drops it silently rather than
/// retrying. Matches `app.js`'s own `PENDING_PUBLISH_MAX_AGE_MS` exactly.
const PENDING_PUBLISH_MAX_AGE_MS: i64 = 5 * 60 * 1000;

/// Hard cap so a long offline stretch can't grow the queue unboundedly.
/// Matches `app.js`'s own `PENDING_PUBLISH_MAX_ENTRIES` exactly.
const PENDING_PUBLISH_MAX_ENTRIES: usize = 50;

/// One publish still outstanding at one or more relays. `relays` narrows
/// down in place as acks arrive via [`record_publish_ack`] — the entry is
/// dropped the moment it empties, in that function, not left for
/// [`due_for_retry`] to notice.
struct PendingPublish {
    event_id: String,
    event_json: String,
    relays: HashSet<String>,
    created_at_ms: i64,
}

/// A field of `crate::AppState`, not its own separately-locked static —
/// same reasoning as [`crate::presence::PresenceState`]'s own doc.
/// `VecDeque`, not a `Vec`: insertion order matters for
/// [`record_pending_publish`]'s evict-oldest bound, same shape as
/// [`crate::nostr_protocol::DedupState`]'s own `order` field.
pub(crate) struct SignalRetryState {
    pending: VecDeque<PendingPublish>,
}

impl SignalRetryState {
    pub(crate) fn new() -> Self {
        SignalRetryState { pending: VecDeque::new() }
    }
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
/// never looks inside it) as needing delivery to `relays` — call right
/// after the shell's own publish attempt, passing only whichever targets
/// it already knows missed (e.g. weren't connected at send time; a relay
/// that *was* sent to but still rejects it is narrowed back in separately,
/// via [`record_publish_ack`]). A no-op for an empty `relays`. Evicts the
/// single oldest entry once the queue exceeds [`PENDING_PUBLISH_MAX_ENTRIES`],
/// same shape as [`crate::nostr_protocol::mark_seen_or_is_duplicate`]'s own
/// bound.
pub fn record_pending_publish(event_id: &str, event_json: &str, relays: &[String], now_ms: i64) {
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
        created_at_ms: now_ms,
    });
}

/// Narrows an existing pending entry for `event_id` by one relay's own
/// outcome: `success` drops `relay` from its still-outstanding set
/// (dropping the whole entry once empty); a failure adds `relay` back in
/// (a no-op if it was already there — e.g. a relay that was both missing
/// at send time and then also explicitly rejected). A no-op if `event_id`
/// isn't currently pending at all — already resolved, already pruned, or
/// never queued because every target was reachable at send time.
pub fn record_publish_ack(event_id: &str, relay: &str, success: bool) {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.signal_retry;
    let Some(entry) = state.pending.iter_mut().find(|p| p.event_id == event_id) else { return };
    if success {
        entry.relays.remove(relay);
        if entry.relays.is_empty() {
            state.pending.retain(|p| p.event_id != event_id);
        }
    } else {
        entry.relays.insert(relay.to_string());
    }
}

/// Called from the shell's own existing periodic tick (the same one
/// `check_online_timeouts`/`check_call_timeout` already run on) — prunes
/// anything past [`PENDING_PUBLISH_MAX_AGE_MS`] first, then hands back
/// every remaining entry's own still-outstanding relay set for the shell to
/// actually republish to. Resending to a relay that already has the event
/// is harmless (see this module's own doc).
pub fn due_for_retry(now_ms: i64) -> Vec<PendingRetry> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.signal_retry;
    state.pending.retain(|p| now_ms - p.created_at_ms < PENDING_PUBLISH_MAX_AGE_MS);
    state
        .pending
        .iter()
        .map(|p| PendingRetry { event_json: p.event_json.clone(), relays: p.relays.iter().cloned().collect() })
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

    fn relays(urls: &[&str]) -> Vec<String> {
        urls.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_fresh_pending_publish_is_due_for_retry_immediately() {
        let _guard = reset_state_for_test();
        record_pending_publish("id1", "{\"id\":\"id1\"}", &relays(&["wss://a", "wss://b"]), 1_000);
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
        record_pending_publish("id1", "{}", &[], 1_000);
        assert!(due_for_retry(1_000).is_empty());
    }

    #[test]
    fn a_successful_ack_narrows_the_relay_set_and_drops_the_entry_once_empty() {
        let _guard = reset_state_for_test();
        record_pending_publish("id1", "{}", &relays(&["wss://a", "wss://b"]), 1_000);
        record_publish_ack("id1", "wss://a", true);
        let due = due_for_retry(1_001);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].relays, vec!["wss://b".to_string()]);

        record_publish_ack("id1", "wss://b", true);
        assert!(due_for_retry(1_002).is_empty(), "entry must be gone once every relay acked");
    }

    #[test]
    fn a_failed_ack_adds_the_relay_back_in_even_if_it_wasnt_originally_missing() {
        let _guard = reset_state_for_test();
        // Only "wss://a" was missing at send time -- "wss://b" was sent to
        // but (per a later ack) rejected it.
        record_pending_publish("id1", "{}", &relays(&["wss://a"]), 1_000);
        record_publish_ack("id1", "wss://b", false);
        let due = due_for_retry(1_001);
        assert_eq!(due.len(), 1);
        let mut got = due[0].relays.clone();
        got.sort();
        assert_eq!(got, vec!["wss://a".to_string(), "wss://b".to_string()]);
    }

    #[test]
    fn an_ack_for_an_unknown_event_id_is_a_no_op() {
        let _guard = reset_state_for_test();
        record_publish_ack("never-queued", "wss://a", true);
        record_publish_ack("never-queued", "wss://a", false);
        assert!(due_for_retry(1_000).is_empty());
    }

    #[test]
    fn an_entry_past_max_age_is_dropped_silently() {
        let _guard = reset_state_for_test();
        record_pending_publish("id1", "{}", &relays(&["wss://a"]), 1_000);
        let due = due_for_retry(1_000 + PENDING_PUBLISH_MAX_AGE_MS - 1);
        assert_eq!(due.len(), 1, "not stale yet");
        let due = due_for_retry(1_000 + PENDING_PUBLISH_MAX_AGE_MS);
        assert!(due.is_empty(), "stale now -- strict less-than");
    }

    #[test]
    fn the_queue_evicts_the_oldest_entry_once_full() {
        let _guard = reset_state_for_test();
        for i in 0..PENDING_PUBLISH_MAX_ENTRIES {
            record_pending_publish(&format!("id{i}"), "{}", &relays(&["wss://a"]), 1_000);
        }
        assert_eq!(due_for_retry(1_000).len(), PENDING_PUBLISH_MAX_ENTRIES);

        // One more push evicts "id0", the oldest -- queue size stays capped,
        // and the evicted entry is actually gone, not just uncounted.
        record_pending_publish("idNew", "{}", &relays(&["wss://a"]), 1_000);
        let due = due_for_retry(1_000);
        assert_eq!(due.len(), PENDING_PUBLISH_MAX_ENTRIES);
        record_publish_ack("id0", "wss://a", true);
        assert_eq!(due_for_retry(1_000).len(), PENDING_PUBLISH_MAX_ENTRIES, "id0 was already gone, not emptied just now");
    }
}
