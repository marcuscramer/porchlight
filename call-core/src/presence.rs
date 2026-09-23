//! Pure decision logic for presence/online-tracking and the adaptive
//! heartbeat — `lastSeenAt`/`onlineState`/`pendingCreatedAt` in the old
//! hand-mirrored Kotlin/JS (see `NostrSignalingClient.kt`'s `markSeen`/
//! `setOnline`/`monitorOnlineTimeouts`/`currentHeartbeatIntervalMs`, and
//! `app.js`'s identically-shaped twins). Ported from a line-by-line
//! extraction of both platforms' current implementations.
//!
//! **Per-pairing maps, not a single global slot**: unlike
//! [`crate::call_arbitration`] (one call, device-wide), presence is tracked
//! independently per pairing — many contacts can be online or offline at
//! once — so this module's own state, [`PresenceState`], is a few
//! `HashMap`s. A field of `crate::AppState`, not its own separately-locked
//! static — see that struct's own doc for why.
//!
//! **Calls directly into [`crate::call_arbitration`]**, not back out
//! through the shell — see [`PresenceUpdateResult`] for why: this crossing
//! is now just an ordinary function call passing `&mut crate::AppState`
//! through, so the presence transition and its `call_arbitration`
//! follow-up are atomic (one critical section, not two with a gap between
//! them).
//!
//! **Time is an explicit parameter** (`now_ms: i64`), never read from the
//! system clock internally — keeps `cargo test` fast and deterministic, no
//! real sleeps.
//!
//! **A pairing's presence is one [`PresenceStatus`], not two independent
//! booleans.** Two separate `HashMap<String, bool>`s (`online_state`,
//! `peer_busy`) is a real design gap, not just style: nothing stops them
//! from disagreeing (a pairing marked `peer_busy = true` while
//! `online_state` still says `false`, say) unless every call site
//! remembers to keep the two maps in lockstep. Collapsing both into one
//! `HashMap<String, PresenceStatus>` makes "offline but also busy" a state
//! that cannot be constructed at all — see [`set_status`]/[`ensure_status`]
//! for the single choke point every transition now goes through.

use crate::call_arbitration::CallEffect;
use serde::Serialize;
use std::collections::HashMap;

/// Steady-state heartbeat cadence — matches both platforms exactly.
/// `pub(crate)` so the JNI/WASM bindings can fail closed to this exact
/// value on a panic, rather than hand-duplicating the number.
pub(crate) const HEARTBEAT_INTERVAL_MS: u32 = 25_000;
/// Faster cadence used while a pairing is still pending confirmation and
/// young (see [`current_heartbeat_interval_ms`]).
const FAST_HEARTBEAT_INTERVAL_MS: u32 = 3_000;
/// How long after a pending pairing is first seen the fast cadence applies.
const FAST_HEARTBEAT_WINDOW_MS: i64 = 30_000;
/// How long without a heartbeat before a peer is declared offline by
/// [`check_online_timeouts`].
const ONLINE_TIMEOUT_MS: i64 = 70_000;

/// A pairing's presence, from this device's own point of view — exactly
/// one of three values at any moment, never a combination. `Busy` implies
/// `Online` (a peer can't be busy without being reachable) — callers that
/// only care about plain reachability (e.g. [`is_online`], or
/// `call_arbitration::request_call`'s own "should I send now or defer"
/// decision) treat `Busy` the same as `Online`; only [`is_peer_busy`] draws
/// the finer distinction. See this module's own top-level doc for why this
/// replaced two independent booleans.
#[derive(Clone, Copy, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    Offline,
    Online,
    Busy,
}

pub(crate) struct PresenceState {
    last_seen_at: HashMap<String, i64>,
    status: HashMap<String, PresenceStatus>,
    pending_created_at: HashMap<String, i64>,
}

impl PresenceState {
    pub(crate) fn new() -> Self {
        PresenceState { last_seen_at: HashMap::new(), status: HashMap::new(), pending_created_at: HashMap::new() }
    }
}

/// What the shell should do about a pairing's presence UI — one effect
/// covering all three states (not separate `SetOnline`/`SetBusy` effects,
/// which is exactly the two-independent-booleans shape this module's own
/// top-level doc explains moving away from): the shell sets its own
/// contact's status to whatever [`PresenceStatus`] this carries, full stop,
/// rather than merging two independent flags itself. See this module's own
/// doc for why `Offline` additionally means "clear `connected`" on Android
/// but not on web (an existing, harmless asymmetry, not something this
/// migration fixes).
#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum PresenceEffect {
    SetStatus { pairing_id: String, status: PresenceStatus },
}

/// What every presence-mutating function hands back: the presence-facing
/// effect(s) (0 or 1 for a single-pairing call; 0+ for
/// [`check_online_timeouts`]'s sweep, which can transition several pairings
/// in one call) plus whatever [`crate::call_arbitration`] effects that
/// transition triggered (e.g. a deferred call resolving the moment its
/// peer comes online) — see this module's own top-level doc for why these
/// two are fused into one result rather than the shell wiring them
/// together itself.
#[derive(Serialize, Debug, PartialEq, Default)]
pub struct PresenceUpdateResult {
    pub presence_effects: Vec<PresenceEffect>,
    pub call_effects: Vec<CallEffect>,
}

impl PresenceUpdateResult {
    fn empty() -> Self {
        PresenceUpdateResult::default()
    }

    fn extend(&mut self, other: PresenceUpdateResult) {
        self.presence_effects.extend(other.presence_effects);
        self.call_effects.extend(other.call_effects);
    }
}

/// `Offline`, not "unknown," for a pairing never seen at all — the single
/// read side of [`set_status`]'s map, kept as its own function so every
/// caller below shares one definition of "what does a missing entry mean."
fn current_status(state: &PresenceState, pairing_id: &str) -> PresenceStatus {
    state.status.get(pairing_id).copied().unwrap_or(PresenceStatus::Offline)
}

/// Sets `pairing_id`'s status, returning an effect only if it actually
/// changed — the one place this module ever writes to `state.status`, so
/// "did this transition actually happen" has exactly one answer everywhere
/// (mirrors `transition_online`/`transition_offline`'s old no-repeat guard
/// shape, now for all three states at once instead of two independent
/// guards that could disagree).
fn set_status(state: &mut PresenceState, pairing_id: &str, new_status: PresenceStatus) -> Option<PresenceEffect> {
    if current_status(state, pairing_id) == new_status {
        return None;
    }
    state.status.insert(pairing_id.to_string(), new_status);
    Some(PresenceEffect::SetStatus { pairing_id: pairing_id.to_string(), status: new_status })
}

/// Moves `pairing_id` to `target` (`Online` or `Busy` — never `Offline`,
/// that's [`transition_offline`]'s own job), firing
/// `call_arbitration::handle_peer_online` exactly once, on the
/// `Offline` → non-`Offline` edge, regardless of which of the two `target`
/// actually is: from `call_arbitration`'s perspective, "this peer is
/// reachable enough to resolve a deferred call" is the same fact whether
/// they're merely online or already on a call with someone else — a
/// `"busy"` reply is itself proof of reachability, the same as a heartbeat.
///
/// Shared by [`mark_seen`] (a heartbeat, or any other message, per its own
/// `peer_busy` contract) and [`handle_peer_busy_reply`] — routing both call
/// sites through this one function means `handle_peer_busy_reply` is
/// self-sufficient, not dependent on the shell having already called
/// `mark_seen` first for the same message.
///
/// Takes `&mut crate::AppState` (not just this module's own `PresenceState`)
/// specifically so it can perform the status transition *and* whatever
/// `call_arbitration` follow-up it triggers atomically, in one critical
/// section — both live behind `crate::STATE`'s single shared lock, so
/// calling `call_arbitration::inner_handle_peer_online` here is just an
/// ordinary function call on state this function already holds, not a
/// second lock acquisition.
fn ensure_status(state: &mut crate::AppState, pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, target: PresenceStatus) -> PresenceUpdateResult {
    let was_offline = current_status(&state.presence, pairing_id) == PresenceStatus::Offline;
    let presence_effects: Vec<PresenceEffect> = set_status(&mut state.presence, pairing_id, target).into_iter().collect();
    let call_effects = if was_offline {
        crate::call_arbitration::inner_handle_peer_online(&mut state.call, pairing_id, own_pubkey_hex, peer_pubkey_hex)
    } else {
        Vec::new()
    };
    PresenceUpdateResult { presence_effects, call_effects }
}

/// Transitions `pairing_id` to `Offline` if it wasn't already — a no-op if
/// it was, matching both platforms' old `setOnline`/`setPeerOnline` guard
/// exactly. One unconditional overwrite ([`set_status`] replaces whatever
/// the prior status was — `Online` or `Busy` — in a single write): with
/// one status field there's nothing separate left to clear.
fn transition_offline(state: &mut crate::AppState, pairing_id: &str) -> PresenceUpdateResult {
    let presence_effects: Vec<PresenceEffect> = set_status(&mut state.presence, pairing_id, PresenceStatus::Offline).into_iter().collect();
    if presence_effects.is_empty() {
        return PresenceUpdateResult::empty();
    }
    let call_effects = crate::call_arbitration::inner_handle_peer_left(&mut state.call, pairing_id);
    PresenceUpdateResult { presence_effects, call_effects }
}

/// Mirrors `markSeen`+`setOnline`'s online branch fused together — every
/// dispatched message from a confirmed peer calls this unconditionally
/// first, regardless of message type. Records `last_seen_at`, then moves
/// the pairing to whichever of `Online`/`Busy` this message implies.
///
/// `peer_busy`: the sender's own self-reported "am I currently on a call"
/// status, `Some(_)` only for a `"heartbeat"` message and `None` for every
/// other message type — the shell passes whatever it parsed (or didn't
/// find) straight through here rather than branching on message type
/// itself.
///
/// `None` leaves whatever the pairing's current status already is alone
/// (`Online` stays `Online`, `Busy` stays `Busy`) — except coming from
/// `Offline`, where it's still treated as "this peer is now reachable" and
/// becomes `Online`. Folding the peer's own live status into the heartbeat
/// this module already processes makes a busy indicator continuously
/// correct, rather than a one-shot reactive flag that auto-clears after a
/// fixed timeout regardless of whether the peer is still actually busy.
pub fn mark_seen(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, now_ms: i64, peer_busy: Option<bool>) -> PresenceUpdateResult {
    let mut state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    state.presence.last_seen_at.insert(pairing_id.to_string(), now_ms);
    let target = match peer_busy {
        Some(true) => PresenceStatus::Busy,
        Some(false) => PresenceStatus::Online,
        None if current_status(&state.presence, pairing_id) == PresenceStatus::Offline => PresenceStatus::Online,
        None => current_status(&state.presence, pairing_id),
    };
    ensure_status(&mut state, pairing_id, own_pubkey_hex, peer_pubkey_hex, target)
}

/// Mirrors receiving a `"busy"` reply to our own outgoing call attempt —
/// fuses the things that reply means into one call, the same way
/// [`mark_seen`]/[`handle_leaving_message`] already fuse a presence
/// transition with whatever `call_arbitration` effects it triggers: this
/// peer is confirmed reachable and busy *right now* (via [`ensure_status`],
/// updated immediately rather than waiting for their next heartbeat), and
/// our own claimed call slot must be released
/// ([`crate::call_arbitration::handle_peer_busy`]). `own_pubkey_hex`/
/// `peer_pubkey_hex`: same tie-break inputs [`mark_seen`] already takes,
/// needed here since this function performs its own online transition
/// rather than relying on the shell having already called `mark_seen`
/// first.
pub fn handle_peer_busy_reply(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, call_id: &str) -> PresenceUpdateResult {
    let mut state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let mut result = ensure_status(&mut state, pairing_id, own_pubkey_hex, peer_pubkey_hex, PresenceStatus::Busy);
    result.call_effects.extend(crate::call_arbitration::inner_handle_peer_busy(&mut state.call, pairing_id, call_id));
    result
}

/// `false` for a pairing never reported busy (including one never seen at
/// all) — used by the shell to seed a contact's initial UI state (e.g.
/// after `restartAgent()` rebuilds its contact list from scratch: this
/// module's own state is a process-global Rust static that survives that
/// rebuild, same reasoning as [`is_online`]'s existing use there).
pub fn is_peer_busy(pairing_id: &str) -> bool {
    let state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    current_status(&state.presence, pairing_id) == PresenceStatus::Busy
}

/// Mirrors the `"leaving"` wire-message case exactly: **removes** the
/// `last_seen_at` entry (not just transitions to `Offline` — a real
/// distinction from the timeout sweep, which leaves it and lets a future
/// `mark_seen` overwrite it naturally), then transitions offline.
pub fn handle_leaving_message(pairing_id: &str) -> PresenceUpdateResult {
    let mut state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    state.presence.last_seen_at.remove(pairing_id);
    transition_offline(&mut state, pairing_id)
}

/// Mirrors `monitor`/`monitorOnlineTimeouts`: sweeps `last_seen_at`,
/// transitions offline anything whose last heartbeat is more than
/// [`ONLINE_TIMEOUT_MS`] behind `now_ms`, concatenating every transitioned
/// pairing's own result into one combined [`PresenceUpdateResult`]. The
/// shell calls this once per sweep tick (its own polling cadence, not
/// owned here); zero, one, or several pairings can time out in a single
/// call.
pub fn check_online_timeouts(now_ms: i64) -> PresenceUpdateResult {
    let mut state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let timed_out: Vec<String> = state
        .presence
        .last_seen_at
        .iter()
        .filter(|(_, &seen)| now_ms - seen > ONLINE_TIMEOUT_MS)
        .map(|(pairing_id, _)| pairing_id.clone())
        .collect();

    let mut result = PresenceUpdateResult::empty();
    for pairing_id in timed_out {
        result.extend(transition_offline(&mut state, &pairing_id));
    }
    result
}

/// `false`, not "unknown," for a pairing never seen at all. `true` for
/// `Busy` as well as `Online` — see [`PresenceStatus`]'s own doc: a busy
/// peer is still a reachable one, and callers of this function (e.g.
/// `call_arbitration::request_call`'s own "send now or defer" decision)
/// want "can I reach them at all," not "are they free."
pub fn is_online(pairing_id: &str) -> bool {
    let state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    current_status(&state.presence, pairing_id) != PresenceStatus::Offline
}

/// Mirrors `currentHeartbeatIntervalMs` exactly, including its side effect
/// of seeding `pending_created_at` the first time a given pending id is
/// seen (matching both platforms' `getOrPut`/`entry().or_insert()` shape).
/// Fast cadence applies whenever any deferred call is waiting
/// ([`crate::call_arbitration::any_call_wanted`] — both platforms already
/// call this inline in this exact function today, so this just relocates
/// an existing call, not new coupling) or any pending pairing is still
/// within [`FAST_HEARTBEAT_WINDOW_MS`] of first being seen.
pub fn current_heartbeat_interval_ms(pending_pairing_ids: &[String], now_ms: i64) -> u32 {
    let mut state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    if crate::call_arbitration::inner_any_call_wanted(&state.call) {
        return FAST_HEARTBEAT_INTERVAL_MS;
    }
    for pairing_id in pending_pairing_ids {
        let created_at = *state.presence.pending_created_at.entry(pairing_id.clone()).or_insert(now_ms);
        if now_ms - created_at < FAST_HEARTBEAT_WINDOW_MS {
            return FAST_HEARTBEAT_INTERVAL_MS;
        }
    }
    HEARTBEAT_INTERVAL_MS
}

/// Mirrors `pruneStalePendingCreatedAt`/the inline `retainAll` in
/// Kotlin's `heartbeatTick` — called by the shell at the top of its own
/// heartbeat-tick function, same as today, so `pending_created_at` doesn't
/// grow forever for pairings that got confirmed or deleted.
pub fn prune_stale_pending(current_pending_ids: &[String]) {
    let mut state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    state.presence.pending_created_at.retain(|pairing_id, _| current_pending_ids.contains(pairing_id));
}

/// Clears `last_seen_at`/`status`/`pending_created_at` for a deleted
/// contact. Called from both platforms' `removePairing`, alongside
/// [`crate::call_arbitration::forget_pairing`].
pub fn remove_pairing(pairing_id: &str) {
    let mut state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    state.presence.last_seen_at.remove(pairing_id);
    state.presence.status.remove(pairing_id);
    state.presence.pending_created_at.remove(pairing_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call_arbitration::CallOutcomeReason;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Resets both this module's own `presence` field and
    /// `call_arbitration`'s `call` field of the shared `crate::STATE` to a
    /// clean slate, serialized on `call_arbitration`'s own `TEST_SERIAL` —
    /// this module calls directly into `call_arbitration`, so a test
    /// leaving stray deferred-call/active-call state behind would leak
    /// across module boundaries. Only these two fields are reset, not the
    /// whole shared `AppState` — see `call_arbitration::TEST_SERIAL`'s own
    /// doc for why that's sufficient.
    fn reset_state_for_test() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::call_arbitration::reset_state_for_test();
        crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).presence = PresenceState::new();
        guard
    }

    fn fresh_id() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        format!("test-pairing-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    // --- mark_seen / transitions ---

    #[test]
    fn mark_seen_records_last_seen_and_transitions_online_once() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = mark_seen(&id, "aaa", "bbb", 1_000, None);
        assert_eq!(result.presence_effects, vec![PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Online }]);
        assert!(is_online(&id));

        // Repeated mark_seen while already online: last_seen_at keeps
        // updating (proven via check_online_timeouts below) but no
        // repeated SetStatus effect.
        let result = mark_seen(&id, "aaa", "bbb", 2_000, None);
        assert!(result.presence_effects.is_empty(), "{:?}", result.presence_effects);
    }

    #[test]
    fn is_online_is_false_for_a_pairing_never_seen() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        assert!(!is_online(&id));
    }

    #[test]
    fn mark_seen_resolves_a_deferred_call_via_call_arbitration() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        // "aaa" < "bbb": requesting side wins the tie-break once online.
        let call_result = crate::call_arbitration::request_call(&id, "aaa", "bbb", false);
        assert!(crate::call_arbitration::any_call_wanted());
        let result = mark_seen(&id, "aaa", "bbb", 1_000, None);
        assert!(
            matches!(result.call_effects.as_slice(), [CallEffect::CreateOffer { call_id, .. }] if Some(call_id.clone()) == call_result.call_id),
            "{:?}",
            result.call_effects
        );
        assert!(!crate::call_arbitration::any_call_wanted());
    }

    // --- handle_leaving_message ---

    #[test]
    fn handle_leaving_message_removes_last_seen_and_transitions_offline() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, None);
        let result = handle_leaving_message(&id);
        assert_eq!(result.presence_effects, vec![PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Offline }]);
        assert!(!is_online(&id));

        // last_seen_at was actually removed, not just status flipped: a
        // sweep long after 1_000 must not re-detect this as "just timed
        // out" (no double transition / no extra effect) since there's
        // nothing there to time out anymore.
        let sweep = check_online_timeouts(1_000 + ONLINE_TIMEOUT_MS + 1);
        assert!(sweep.presence_effects.is_empty(), "{:?}", sweep.presence_effects);
    }

    #[test]
    fn handle_leaving_message_ends_an_active_call_via_call_arbitration() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, None);
        crate::call_arbitration::handle_should_offer(&id, "call1", "aaa", "bbb", false);
        let result = handle_leaving_message(&id);
        assert_eq!(
            result.call_effects,
            vec![
                CallEffect::ClearIncomingCallTimer,
                CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::NeverConnected },
                CallEffect::ClosePeerConnection,
            ],
            "{:?}",
            result.call_effects
        );
    }

    // --- check_online_timeouts ---

    #[test]
    fn check_online_timeouts_leaves_a_recently_seen_pairing_online() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, None);
        let result = check_online_timeouts(1_000 + ONLINE_TIMEOUT_MS - 1);
        assert!(result.presence_effects.is_empty());
        assert!(is_online(&id));
    }

    #[test]
    fn check_online_timeouts_transitions_offline_exactly_past_the_threshold() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, None);
        // At exactly the threshold, not yet timed out (strict >).
        let result = check_online_timeouts(1_000 + ONLINE_TIMEOUT_MS);
        assert!(result.presence_effects.is_empty(), "{:?}", result.presence_effects);
        let result = check_online_timeouts(1_000 + ONLINE_TIMEOUT_MS + 1);
        assert_eq!(result.presence_effects, vec![PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Offline }]);
        assert!(!is_online(&id));
    }

    #[test]
    fn check_online_timeouts_handles_several_pairings_timing_out_in_one_sweep() {
        let _guard = reset_state_for_test();
        let id_a = fresh_id();
        let id_b = fresh_id();
        let id_c = fresh_id();
        mark_seen(&id_a, "aaa", "111", 1_000, None);
        mark_seen(&id_b, "aaa", "222", 1_000, None);
        mark_seen(&id_c, "aaa", "333", 50_000, None); // seen much later, stays online

        let result = check_online_timeouts(1_000 + ONLINE_TIMEOUT_MS + 1);
        let mut transitioned: Vec<String> =
            result.presence_effects.iter().map(|PresenceEffect::SetStatus { pairing_id, .. }| pairing_id.clone()).collect();
        transitioned.sort();
        let mut expected = vec![id_a.clone(), id_b.clone()];
        expected.sort();
        assert_eq!(transitioned, expected);
        assert!(is_online(&id_c));
    }

    // --- current_heartbeat_interval_ms ---

    #[test]
    fn current_heartbeat_interval_ms_is_slow_with_no_pending_and_no_wanted_call() {
        let _guard = reset_state_for_test();
        assert_eq!(current_heartbeat_interval_ms(&[], 1_000), HEARTBEAT_INTERVAL_MS);
    }

    #[test]
    fn current_heartbeat_interval_ms_is_fast_when_a_call_is_wanted() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        crate::call_arbitration::request_call(&id, "aaa", "bbb", false);
        assert_eq!(current_heartbeat_interval_ms(&[], 1_000), FAST_HEARTBEAT_INTERVAL_MS);
    }

    #[test]
    fn current_heartbeat_interval_ms_is_fast_within_window_of_a_pending_pairing_then_slows() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let pending = vec![id.clone()];
        // First call seeds pending_created_at at now=1_000.
        assert_eq!(current_heartbeat_interval_ms(&pending, 1_000), FAST_HEARTBEAT_INTERVAL_MS);
        // Still within the window.
        assert_eq!(current_heartbeat_interval_ms(&pending, 1_000 + FAST_HEARTBEAT_WINDOW_MS - 1), FAST_HEARTBEAT_INTERVAL_MS);
        // Past the window: slow, and the seeded created_at wasn't reset by
        // the calls above (proven by this now actually being slow).
        assert_eq!(current_heartbeat_interval_ms(&pending, 1_000 + FAST_HEARTBEAT_WINDOW_MS + 1), HEARTBEAT_INTERVAL_MS);
    }

    // --- prune_stale_pending ---

    #[test]
    fn prune_stale_pending_removes_entries_not_in_the_current_list() {
        let _guard = reset_state_for_test();
        let id_a = fresh_id();
        let id_b = fresh_id();
        current_heartbeat_interval_ms(&[id_a.clone(), id_b.clone()], 1_000);
        prune_stale_pending(std::slice::from_ref(&id_a));
        // id_b's pending_created_at was pruned, so it re-seeds fresh "now"
        // rather than reusing the old timestamp — proven by it counting as
        // fast again arbitrarily far past the original window.
        let far_future = 1_000 + FAST_HEARTBEAT_WINDOW_MS * 100;
        assert_eq!(current_heartbeat_interval_ms(std::slice::from_ref(&id_b), far_future), FAST_HEARTBEAT_INTERVAL_MS);
    }

    // --- remove_pairing ---

    #[test]
    fn remove_pairing_clears_all_three_maps() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, None);
        current_heartbeat_interval_ms(std::slice::from_ref(&id), 1_000);
        assert!(is_online(&id));

        remove_pairing(&id);
        assert!(!is_online(&id));
        // last_seen_at cleared: a sweep long after must find nothing to
        // time out (no effect emitted) for this id.
        let sweep = check_online_timeouts(1_000 + ONLINE_TIMEOUT_MS + 1);
        assert!(sweep.presence_effects.is_empty(), "{:?}", sweep.presence_effects);
        // pending_created_at cleared: re-seeds fresh rather than reusing
        // the old (now long-past-window) timestamp.
        let far_future = 1_000 + FAST_HEARTBEAT_WINDOW_MS * 100;
        assert_eq!(current_heartbeat_interval_ms(std::slice::from_ref(&id), far_future), FAST_HEARTBEAT_INTERVAL_MS);
    }

    #[test]
    fn remove_pairing_with_an_in_flight_deferred_call_also_needs_forget_pairing() {
        let _guard = reset_state_for_test();
        // This test documents the ordering contract from the plan: presence's
        // own remove_pairing does NOT touch call_arbitration state (it isn't
        // presence's to own) — the shell must call
        // call_arbitration::forget_pairing alongside it. Verify remove_pairing
        // alone leaves a deferred call's wants_call entry untouched.
        let id = fresh_id();
        crate::call_arbitration::request_call(&id, "aaa", "bbb", false);
        assert!(crate::call_arbitration::any_call_wanted());
        remove_pairing(&id);
        assert!(crate::call_arbitration::any_call_wanted(), "remove_pairing alone must not clear call_arbitration state");
        crate::call_arbitration::forget_pairing(&id);
        assert!(!crate::call_arbitration::any_call_wanted());
    }

    // --- live busy status (mark_seen's peer_busy param / handle_peer_busy_reply) ---

    #[test]
    fn mark_seen_with_peer_busy_true_emits_set_status_busy_and_sticks() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = mark_seen(&id, "aaa", "bbb", 1_000, Some(true));
        assert!(
            result.presence_effects.contains(&PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Busy }),
            "{:?}",
            result.presence_effects
        );
        assert!(is_peer_busy(&id));

        // A second heartbeat still reporting busy=true is not a change —
        // no repeated effect, matching the plain-online case's own
        // no-repeat guard shape.
        let result = mark_seen(&id, "aaa", "bbb", 2_000, Some(true));
        assert!(result.presence_effects.is_empty(), "{:?}", result.presence_effects);
        assert!(is_peer_busy(&id));
    }

    #[test]
    fn mark_seen_with_peer_busy_none_never_touches_busy_state() {
        // None is what every non-heartbeat message type passes (see
        // mark_seen's own doc) — must not spuriously flip busy either way.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, Some(true));
        assert!(is_peer_busy(&id));
        let result = mark_seen(&id, "aaa", "bbb", 2_000, None);
        assert!(result.presence_effects.is_empty(), "{:?}", result.presence_effects);
        assert!(is_peer_busy(&id), "status must be left exactly as it was when the message carries no info about busy");
    }

    #[test]
    fn mark_seen_with_peer_busy_false_after_true_clears_it_live() {
        // The whole point of this feature: a later heartbeat correctly
        // reporting the peer is no longer busy actually updates the
        // status, unlike the old one-shot-plus-timeout approximation.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, Some(true));
        assert!(is_peer_busy(&id));
        let result = mark_seen(&id, "aaa", "bbb", 2_000, Some(false));
        assert_eq!(result.presence_effects, vec![PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Online }]);
        assert!(!is_peer_busy(&id));
        assert!(is_online(&id), "dropping busy must land on Online, not Offline");
    }

    #[test]
    fn is_peer_busy_is_false_for_a_pairing_never_reported_busy() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        assert!(!is_peer_busy(&id));
    }

    #[test]
    fn handle_peer_busy_reply_sets_busy_immediately_and_releases_the_callers_slot() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = crate::call_arbitration::request_call(&id, "aaa", "bbb", true);
        let call_id = result.call_id.unwrap();
        assert!(!is_peer_busy(&id), "not busy yet -- no heartbeat or busy reply has arrived");

        let result = handle_peer_busy_reply(&id, "aaa", "bbb", &call_id);
        assert_eq!(result.presence_effects, vec![PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Busy }]);
        assert!(is_peer_busy(&id), "must be immediate, not wait for the peer's next heartbeat");
        assert!(!result.call_effects.is_empty(), "must also release the caller's own claimed slot via call_arbitration::handle_peer_busy: {:?}", result.call_effects);
        assert!(!crate::call_arbitration::any_call_wanted());
    }

    #[test]
    fn handle_peer_busy_reply_transitions_a_still_offline_pairing_structurally_not_by_convention() {
        // A "busy" reply must be able to stand on its own -- correctly
        // marking the pairing reachable-and-busy -- even if, for whatever
        // reason, the shell's own mark_seen call for this same message
        // never ran or hasn't run yet.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        assert!(!is_online(&id));
        let result = handle_peer_busy_reply(&id, "aaa", "bbb", "some-call-id");
        assert_eq!(result.presence_effects, vec![PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Busy }]);
        assert!(is_online(&id), "Busy must imply Online -- there is no representable Offline-but-busy state");
        assert!(is_peer_busy(&id));
    }

    #[test]
    fn handle_peer_busy_reply_is_a_no_op_call_effect_wise_when_ids_dont_match() {
        // Same gating as handle_peer_busy itself (both pairing_id and
        // call_id must match the currently active attempt) -- a stale/
        // mismatched busy reply must not release a slot that isn't
        // actually about it. The presence-side busy status still updates
        // regardless -- that half has nothing to do with which call
        // attempt this device itself has active. No wants_call was ever
        // registered for this pairing either, so the online transition
        // this reply also performs (see ensure_status) contributes no
        // call_arbitration effects of its own.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = handle_peer_busy_reply(&id, "aaa", "bbb", "some-other-call-id");
        assert_eq!(result.presence_effects, vec![PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Busy }]);
        assert!(result.call_effects.is_empty(), "{:?}", result.call_effects);
    }

    #[test]
    fn transition_offline_clears_a_busy_status_in_one_step() {
        let _guard = reset_state_for_test();
        let busy_id = fresh_id();
        let never_busy_id = fresh_id();
        mark_seen(&busy_id, "aaa", "bbb", 1_000, Some(true));
        mark_seen(&never_busy_id, "aaa", "ccc", 1_000, None);
        assert!(is_peer_busy(&busy_id));
        assert!(!is_peer_busy(&never_busy_id));

        // handle_leaving_message drives transition_offline directly.
        let result = handle_leaving_message(&busy_id);
        assert_eq!(result.presence_effects, vec![PresenceEffect::SetStatus { pairing_id: busy_id.clone(), status: PresenceStatus::Offline }]);
        assert!(!is_peer_busy(&busy_id));
        assert!(!is_online(&busy_id));

        // The never-busy pairing going offline emits exactly the same
        // single kind of effect -- no separate busy-clearing step exists
        // to possibly skip or duplicate.
        let result = handle_leaving_message(&never_busy_id);
        assert_eq!(result.presence_effects, vec![PresenceEffect::SetStatus { pairing_id: never_busy_id.clone(), status: PresenceStatus::Offline }]);
    }

    #[test]
    fn remove_pairing_clears_peer_busy_too() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, Some(true));
        assert!(is_peer_busy(&id));
        remove_pairing(&id);
        assert!(!is_peer_busy(&id));
    }
}
