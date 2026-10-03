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
//!
//! # Invariants
//!
//! - **A pairing's presence is one `PresenceStatus`, and `Busy` implies
//!   `Online` structurally.** `handle_peer_busy_reply` does its own
//!   `Offline` -> `Busy` transition through the single choke point rather
//!   than assuming the shell already called `mark_seen` for the same
//!   message. Test:
//!   `handle_peer_busy_reply_transitions_a_still_offline_pairing_structurally_not_by_convention`.
//! - **A `leaving` message *removes* `last_seen_at`; the timeout sweep only
//!   leaves it stale** for a later `mark_seen` to overwrite. Not
//!   interchangeable. Test:
//!   `handle_leaving_message_removes_last_seen_and_transitions_offline`.
//! - **Deleting a pairing takes two calls**: `remove_pairing` clears this
//!   module's own state but can't (and shouldn't) touch
//!   `call_arbitration`'s deferred-call state, so both shells' removal also
//!   calls `call_arbitration::forget_pairing`. Test:
//!   `remove_pairing_with_an_in_flight_deferred_call_also_needs_forget_pairing`.
//! - **A `hello` is answered at most once per pairing per
//!   `HELLO_REPLY_MIN_INTERVAL_MS`, and the answer never carries `hello`
//!   itself** — two devices can't ping-pong however they misbehave. Tests:
//!   `hello_replies_are_rate_limited_per_pairing`,
//!   `a_reload_hello_is_answered_once_and_the_answer_does_not_ask_for_another`.
//! - **Nothing speeds the heartbeat up for a call.** A call placed to an
//!   offline-looking peer asks for exactly one prompt `hello` heartbeat
//!   (`CallEffect::KickHeartbeat`); only a live pairing attempt shortens the
//!   tick. Tests: `a_deferred_call_does_not_speed_the_heartbeat_up`,
//!   `a_live_pairing_attempt_uses_the_pairing_cadence_a_dead_one_does_not`.

use crate::call_arbitration::CallEffect;
use serde::Serialize;
use std::collections::HashMap;

/// Steady-state heartbeat cadence — matches both platforms exactly.
/// `pub(crate)` so the JNI/WASM bindings can fail closed to this exact
/// value on a panic, rather than hand-duplicating the number.
pub(crate) const HEARTBEAT_INTERVAL_MS: u32 = 25_000;
/// Cadence while a pairing attempt is live (see
/// [`current_heartbeat_interval_ms`]). Not the main mechanism for getting a
/// joiner through quickly — answering a newcomer immediately is
/// (`Effect::KickHeartbeat` on first sight of a candidate, see
/// `handle_bootstrap_message`) — but the backstop for a lost message, and
/// the only thing that gets a pairing message out when the *other* side is
/// the one who has to speak first. Used to be 3s for the first 30s and then
/// the steady 25s, which both wasted traffic early and left a late joiner
/// waiting out 25s.
const PAIRING_REPUBLISH_INTERVAL_MS: u32 = 10_000;
/// How long without a heartbeat before a peer is declared offline by
/// [`check_online_timeouts`].
const ONLINE_TIMEOUT_MS: i64 = 70_000;
/// Minimum gap between two "reply to this hello" effects for one pairing —
/// a hello is answered with an immediate heartbeat of our own (see
/// [`mark_seen`]), and a peer that (by bug or malice) set `hello` on every
/// heartbeat must not turn that into a ping-pong or a flood.
const HELLO_REPLY_MIN_INTERVAL_MS: i64 = 5_000;

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
    last_hello_reply_at: HashMap<String, i64>,
    /// Whether the next heartbeat this device builds should carry `hello`.
    /// Starts `true`: a freshly started process (a browser reload, an app
    /// restart) knows nothing about any peer, and every presence status it
    /// holds is "never seen", not "offline".
    hello_pending: bool,
}

impl PresenceState {
    /// Same as [`request_hello`], for a caller already holding the state lock.
    pub(crate) fn request_hello(&mut self) {
        self.hello_pending = true;
    }

    pub(crate) fn new() -> Self {
        PresenceState {
            last_seen_at: HashMap::new(),
            status: HashMap::new(),
            last_hello_reply_at: HashMap::new(),
            hello_pending: true,
        }
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
    /// This peer's heartbeat said `hello` (see [`mark_seen`]): the shell
    /// should send *that pairing* one ordinary heartbeat right now, outside
    /// its normal schedule. Not a UI effect — the signaling layer handles it.
    ReplyHeartbeat { pairing_id: String },
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
fn ensure_status(state: &mut crate::AppState, pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, target: PresenceStatus, now_ms: i64) -> PresenceUpdateResult {
    let was_offline = current_status(&state.presence, pairing_id) == PresenceStatus::Offline;
    let presence_effects: Vec<PresenceEffect> = set_status(&mut state.presence, pairing_id, target).into_iter().collect();
    let call_effects = if was_offline {
        crate::call_arbitration::inner_handle_peer_online(&mut state.call, pairing_id, own_pubkey_hex, peer_pubkey_hex, now_ms)
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
///
/// `peer_hello`: the heartbeat carried `hello` — the sender just (re)started
/// or regained connectivity and has no idea who is online. Presence is
/// otherwise learned only from the peer's *own* periodic heartbeat (slow,
/// by design), so without an answer a freshly loaded device would show
/// everyone offline for up to a full heartbeat interval. Answer with one
/// immediate heartbeat ([`PresenceEffect::ReplyHeartbeat`]), at most once per
/// [`HELLO_REPLY_MIN_INTERVAL_MS`] per pairing. The reply itself carries no
/// `hello`, so two devices can never ping-pong.
pub fn mark_seen(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, now_ms: i64, peer_busy: Option<bool>, peer_hello: bool) -> PresenceUpdateResult {
    let mut state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    state.presence.last_seen_at.insert(pairing_id.to_string(), now_ms);
    let target = match peer_busy {
        Some(true) => PresenceStatus::Busy,
        Some(false) => PresenceStatus::Online,
        None if current_status(&state.presence, pairing_id) == PresenceStatus::Offline => PresenceStatus::Online,
        None => current_status(&state.presence, pairing_id),
    };
    let mut result = ensure_status(&mut state, pairing_id, own_pubkey_hex, peer_pubkey_hex, target, now_ms);
    if peer_hello {
        let due = state.presence.last_hello_reply_at.get(pairing_id).is_none_or(|&last| now_ms - last >= HELLO_REPLY_MIN_INTERVAL_MS);
        if due {
            state.presence.last_hello_reply_at.insert(pairing_id.to_string(), now_ms);
            result.presence_effects.push(PresenceEffect::ReplyHeartbeat { pairing_id: pairing_id.to_string() });
        }
    }
    result
}

/// Marks the next [`crate::nostr_protocol::build_heartbeat_payload`] as a
/// `hello` — call when this device regains signaling connectivity or the
/// user comes back to a backgrounded page, i.e. whenever what it believes
/// about its peers may be stale or empty. See [`mark_seen`].
pub fn request_hello() {
    crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).presence.hello_pending = true;
}

/// Whether the heartbeat being built right now should carry `hello` —
/// consumed: it's `true` once per [`request_hello`] (and once at process
/// start), so the shell builds one heartbeat per tick and that single
/// payload goes to every peer.
pub fn take_hello() -> bool {
    std::mem::take(&mut crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).presence.hello_pending)
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
pub fn handle_peer_busy_reply(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, call_id: &str, now_ms: i64) -> PresenceUpdateResult {
    let mut state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let mut result = ensure_status(&mut state, pairing_id, own_pubkey_hex, peer_pubkey_hex, PresenceStatus::Busy, now_ms);
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

/// The delay before the next heartbeat tick: [`PAIRING_REPUBLISH_INTERVAL_MS`]
/// while any of `pending_pairing_ids` has a live pairing attempt (its
/// bootstrap messages are republished on the same tick), otherwise the
/// steady [`HEARTBEAT_INTERVAL_MS`]. Everything else that wants a prompt
/// heartbeat asks for exactly one — a `hello` (see [`mark_seen`]) or an
/// explicit kick — instead of raising the rate: a deferred call used to
/// speed everything up to 3s even though our own heartbeats do nothing to
/// make the peer answer sooner.
pub fn current_heartbeat_interval_ms(pending_pairing_ids: &[String]) -> u32 {
    let state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    if pending_pairing_ids.iter().any(|id| state.pairing_registry.contains_key(id)) {
        return PAIRING_REPUBLISH_INTERVAL_MS;
    }
    HEARTBEAT_INTERVAL_MS
}

/// Clears `last_seen_at`/`status`/`last_hello_reply_at` for a deleted
/// contact. Called from both platforms' `removePairing`, alongside
/// [`crate::call_arbitration::forget_pairing`].
pub fn remove_pairing(pairing_id: &str) {
    let mut state = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    state.presence.last_seen_at.remove(pairing_id);
    state.presence.status.remove(pairing_id);
    state.presence.last_hello_reply_at.remove(pairing_id);
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
        let result = mark_seen(&id, "aaa", "bbb", 1_000, None, false);
        assert_eq!(result.presence_effects, vec![PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Online }]);
        assert!(is_online(&id));

        // Repeated mark_seen while already online: last_seen_at keeps
        // updating (proven via check_online_timeouts below) but no
        // repeated SetStatus effect.
        let result = mark_seen(&id, "aaa", "bbb", 2_000, None, false);
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
        let call_result = crate::call_arbitration::request_call(&id, "aaa", "bbb", false, 0);
        assert!(crate::call_arbitration::any_call_wanted());
        let result = mark_seen(&id, "aaa", "bbb", 1_000, None, false);
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
        mark_seen(&id, "aaa", "bbb", 1_000, None, false);
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
        mark_seen(&id, "aaa", "bbb", 1_000, None, false);
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
        mark_seen(&id, "aaa", "bbb", 1_000, None, false);
        let result = check_online_timeouts(1_000 + ONLINE_TIMEOUT_MS - 1);
        assert!(result.presence_effects.is_empty());
        assert!(is_online(&id));
    }

    #[test]
    fn check_online_timeouts_transitions_offline_exactly_past_the_threshold() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, None, false);
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
        mark_seen(&id_a, "aaa", "111", 1_000, None, false);
        mark_seen(&id_b, "aaa", "222", 1_000, None, false);
        mark_seen(&id_c, "aaa", "333", 50_000, None, false); // seen much later, stays online

        let result = check_online_timeouts(1_000 + ONLINE_TIMEOUT_MS + 1);
        let mut transitioned: Vec<String> =
            result.presence_effects.iter().filter_map(|e| match e { PresenceEffect::SetStatus { pairing_id, .. } => Some(pairing_id.clone()), _ => None }).collect();
        transitioned.sort();
        let mut expected = vec![id_a.clone(), id_b.clone()];
        expected.sort();
        assert_eq!(transitioned, expected);
        assert!(is_online(&id_c));
    }

    // --- current_heartbeat_interval_ms ---

    #[test]
    fn current_heartbeat_interval_ms_is_steady_with_nothing_pending() {
        let _guard = reset_state_for_test();
        assert_eq!(current_heartbeat_interval_ms(&[]), HEARTBEAT_INTERVAL_MS);
    }

    #[test]
    fn a_deferred_call_does_not_speed_the_heartbeat_up() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        crate::call_arbitration::request_call(&id, "aaa", "bbb", false, 0);
        assert_eq!(current_heartbeat_interval_ms(&[]), HEARTBEAT_INTERVAL_MS);
    }

    #[test]
    fn a_live_pairing_attempt_uses_the_pairing_cadence_a_dead_one_does_not() {
        let _guard = reset_state_for_test();
        let live = fresh_id();
        let dead = fresh_id();
        crate::start_attempt(&live, "pubkey-a", "A", "cadence test");
        assert_eq!(current_heartbeat_interval_ms(std::slice::from_ref(&live)), PAIRING_REPUBLISH_INTERVAL_MS);
        assert_eq!(current_heartbeat_interval_ms(std::slice::from_ref(&dead)), HEARTBEAT_INTERVAL_MS, "a pending pairing with no live attempt has nothing to republish");
        crate::cancel_attempt(&live);
        assert_eq!(current_heartbeat_interval_ms(std::slice::from_ref(&live)), HEARTBEAT_INTERVAL_MS);
    }

    // --- remove_pairing ---

    #[test]
    fn remove_pairing_clears_the_pairings_presence_state() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, None, false);
        assert!(is_online(&id));

        remove_pairing(&id);
        assert!(!is_online(&id));
        // last_seen_at cleared: a sweep long after must find nothing to
        // time out (no effect emitted) for this id.
        let sweep = check_online_timeouts(1_000 + ONLINE_TIMEOUT_MS + 1);
        assert!(sweep.presence_effects.is_empty(), "{:?}", sweep.presence_effects);
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
        crate::call_arbitration::request_call(&id, "aaa", "bbb", false, 0);
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
        let result = mark_seen(&id, "aaa", "bbb", 1_000, Some(true), false);
        assert!(
            result.presence_effects.contains(&PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Busy }),
            "{:?}",
            result.presence_effects
        );
        assert!(is_peer_busy(&id));

        // A second heartbeat still reporting busy=true is not a change —
        // no repeated effect, matching the plain-online case's own
        // no-repeat guard shape.
        let result = mark_seen(&id, "aaa", "bbb", 2_000, Some(true), false);
        assert!(result.presence_effects.is_empty(), "{:?}", result.presence_effects);
        assert!(is_peer_busy(&id));
    }

    #[test]
    fn mark_seen_with_peer_busy_none_never_touches_busy_state() {
        // None is what every non-heartbeat message type passes (see
        // mark_seen's own doc) — must not spuriously flip busy either way.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, Some(true), false);
        assert!(is_peer_busy(&id));
        let result = mark_seen(&id, "aaa", "bbb", 2_000, None, false);
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
        mark_seen(&id, "aaa", "bbb", 1_000, Some(true), false);
        assert!(is_peer_busy(&id));
        let result = mark_seen(&id, "aaa", "bbb", 2_000, Some(false), false);
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
        let result = crate::call_arbitration::request_call(&id, "aaa", "bbb", true, 0);
        let call_id = result.call_id.unwrap();
        assert!(!is_peer_busy(&id), "not busy yet -- no heartbeat or busy reply has arrived");

        let result = handle_peer_busy_reply(&id, "aaa", "bbb", &call_id, 0);
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
        let result = handle_peer_busy_reply(&id, "aaa", "bbb", "some-call-id", 0);
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
        let result = handle_peer_busy_reply(&id, "aaa", "bbb", "some-other-call-id", 0);
        assert_eq!(result.presence_effects, vec![PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Busy }]);
        assert!(result.call_effects.is_empty(), "{:?}", result.call_effects);
    }

    #[test]
    fn transition_offline_clears_a_busy_status_in_one_step() {
        let _guard = reset_state_for_test();
        let busy_id = fresh_id();
        let never_busy_id = fresh_id();
        mark_seen(&busy_id, "aaa", "bbb", 1_000, Some(true), false);
        mark_seen(&never_busy_id, "aaa", "ccc", 1_000, None, false);
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
        mark_seen(&id, "aaa", "bbb", 1_000, Some(true), false);
        assert!(is_peer_busy(&id));
        remove_pairing(&id);
        assert!(!is_peer_busy(&id));
    }

    // --- hello / reply ---

    #[test]
    fn a_hello_heartbeat_gets_one_immediate_reply_effect() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = mark_seen(&id, "aaa", "bbb", 1_000, Some(false), true);
        assert_eq!(
            result.presence_effects,
            vec![
                PresenceEffect::SetStatus { pairing_id: id.clone(), status: PresenceStatus::Online },
                PresenceEffect::ReplyHeartbeat { pairing_id: id.clone() },
            ]
        );
    }

    #[test]
    fn an_ordinary_heartbeat_never_gets_a_reply() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, Some(false), false);
        let result = mark_seen(&id, "aaa", "bbb", 2_000, Some(false), false);
        assert!(result.presence_effects.is_empty(), "{:?}", result.presence_effects);
    }

    #[test]
    fn hello_replies_are_rate_limited_per_pairing() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let other = fresh_id();
        let reply = |r: &PresenceUpdateResult| r.presence_effects.iter().any(|e| matches!(e, PresenceEffect::ReplyHeartbeat { .. }));
        assert!(reply(&mark_seen(&id, "aaa", "bbb", 1_000, Some(false), true)));
        assert!(!reply(&mark_seen(&id, "aaa", "bbb", 1_000 + HELLO_REPLY_MIN_INTERVAL_MS - 1, Some(false), true)), "too soon");
        assert!(reply(&mark_seen(&other, "aaa", "ccc", 1_001, Some(false), true)), "a different pairing has its own budget");
        assert!(reply(&mark_seen(&id, "aaa", "bbb", 1_000 + HELLO_REPLY_MIN_INTERVAL_MS, Some(false), true)), "budget refilled");
    }

    #[test]
    fn hello_is_pending_at_start_is_consumed_once_and_can_be_requested_again() {
        let _guard = reset_state_for_test();
        assert!(take_hello(), "a fresh process starts out wanting a hello");
        assert!(!take_hello(), "consumed");
        request_hello();
        assert!(take_hello());
        assert!(!take_hello());
    }

    #[test]
    fn remove_pairing_forgets_the_hello_reply_budget() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        mark_seen(&id, "aaa", "bbb", 1_000, Some(false), true);
        remove_pairing(&id);
        let result = mark_seen(&id, "aaa", "bbb", 1_001, Some(false), true);
        assert!(result.presence_effects.iter().any(|e| matches!(e, PresenceEffect::ReplyHeartbeat { .. })));
    }

    #[test]
    fn a_reload_hello_is_answered_once_and_the_answer_does_not_ask_for_another() {
        use crate::nostr_protocol::{build_heartbeat_payload, parse_signal_payload, SignalMessage};
        let _guard = reset_state_for_test();
        let id = fresh_id();

        // The freshly loaded device's first heartbeat says hello...
        let hello = build_heartbeat_payload("Browser", false).unwrap();
        let SignalMessage::Heartbeat { hello: is_hello, .. } = parse_signal_payload(&hello).unwrap() else { panic!("not a heartbeat") };
        assert!(is_hello);

        // ...the peer's receipt of it asks for an immediate reply...
        let result = mark_seen(&id, "aaa", "bbb", 1_000, Some(false), is_hello);
        assert!(result.presence_effects.contains(&PresenceEffect::ReplyHeartbeat { pairing_id: id.clone() }));

        // ...and that reply (built right after, as the shell does) is an
        // ordinary heartbeat: it neither says hello nor can trigger a reply.
        let reply = build_heartbeat_payload("Portal", false).unwrap();
        assert!(!reply.contains("hello"), "{reply}");
        let SignalMessage::Heartbeat { hello: reply_hello, .. } = parse_signal_payload(&reply).unwrap() else { panic!("not a heartbeat") };
        assert!(!mark_seen(&id, "bbb", "aaa", 1_500, Some(false), reply_hello).presence_effects.iter().any(|e| matches!(e, PresenceEffect::ReplyHeartbeat { .. })));
    }
}
