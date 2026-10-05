//! Pure decision logic for the call-arbitration state machine — the one
//! implementation, shared by Android (`CallCoreBridge.kt`) and web (`app.js`,
//! via the WASM build), both of which are thin shells that forward events in
//! and execute whatever `CallEffect`s come back. Ported from what used to be
//! two hand-mirrored copies; see the **Invariants** section below for the
//! rules this has to keep, and **Changing this module** for how.
//!
//! **Single global slot, not a per-id registry**: only one call is ever
//! active device-wide, regardless of how many pairings exist, so
//! `CallState` is one struct, not a `HashMap`.
//!
//! Lives behind `crate::STATE`'s shared lock, not one of its own.
//! [`request_call`]/[`handle_offer`]/etc. stay the locking entry points the
//! shell calls; a few functions `presence` needs to call while already
//! holding the lock have a `pub(crate) inner_*` twin that takes
//! `&mut CallState` directly instead of re-locking.
//!
//! **What this module doesn't own**: the real `PeerConnection` object;
//! `hasActivePeerConnection` (passed in, snapshotted by the shell, never
//! inferred here); wake locks/foreground-service bring-up (Android-only);
//! and presence/online tracking itself — this module only owns what happens
//! *once* a peer is known to be online, via [`handle_peer_online`].
//!
//! # The state
//!
//! `CallState`'s `slot` is the device's one call slot, exactly one of:
//!
//! - `Idle` — nothing claimed.
//! - `Ringing(PendingOffer)` — an incoming offer has arrived but hasn't been
//!   handed to the `PeerConnection` yet: counting down (auto-answer) or
//!   waiting on a human tap. Holds the raw SDP plus an `ice_buffer` for ICE
//!   candidates that arrive during that window (trickle ICE means the caller
//!   can be sending them before ringing resolves, and the `PeerConnection`
//!   doesn't exist yet to hand them to — see `handle_remote_ice`'s
//!   `Buffered` outcome).
//! - `Claimed(ActiveCall)` — a call is placed or being negotiated:
//!   `pairing_id` and `call_id` (distinct because a pairing can be called
//!   again right after an attempt ends; every offer/answer/ice/bye is tagged
//!   with `call_id`, and anything arriving with a different one for the same
//!   pairing is a stale or abandoned attempt, not the current call), plus
//!   `answer_applied`/`offer_applied`/`connected_once`.
//!
//! Only one pairing can hold the slot (any variant but `Idle`) at a time; a
//! second pairing trying to call in meanwhile gets a `busy` reply
//! (`handle_should_offer`/`handle_offer`'s first guard). The `PeerConnection`
//! itself is deliberately **not** owned here — only
//! `has_active_peer_connection: bool` where a function still needs it, passed
//! in by the shell. It exists only once an offer has actually been applied,
//! i.e. *after* ringing resolves: the line between "a slot is reserved" and
//! "a call is actually being negotiated".
//!
//! # Invariants
//!
//! Each is enforced by a specific function here and pinned by the named
//! test(s). Three real bugs shipped in one feature (per-contact auto-answer)
//! because these rules existed only as scattered comments — this list is the
//! one place a change can check itself against. A violation is fixed *here*
//! (then `cargo test`), and both platforms inherit it; never hand-patch one
//! shell and port it later.
//!
//! 1. **The slot is claimed the moment a call *could* happen, not once it's
//!    accepted.** `request_call`, `handle_offer` and `handle_should_offer`
//!    all move the slot out of `Idle` immediately — before any human has
//!    decided — so a second contact calling during that window gets `busy`
//!    instead of silently racing the first. Don't gate the claim on
//!    acceptance. Tests:
//!    `request_call_claims_slot_and_offers_when_peer_online_and_own_pubkey_wins_tiebreak`,
//!    `handle_should_offer_claims_slot_and_rings_respecting_auto_answer`,
//!    `handle_offer_fresh_call_starts_ringing_not_apply`.
//! 2. **`Ringing` is the *only* thing acceptance gates.** The
//!    `PeerConnection` must not exist until a real accept (manual tap,
//!    countdown reaching zero, or the tie-break bypass in #3). Anything that
//!    needs "is this call really live" must check the `PeerConnection`, not
//!    whether the slot is merely claimed. Tests:
//!    `handle_offer_fresh_call_starts_ringing_not_apply`,
//!    `accept_incoming_call_hands_back_offer_and_buffered_ice_then_clears_pending`.
//! 3. **An offer that completes a call *this side* already placed must skip
//!    ringing.** The side that loses the pubkey tie-break sends a plain
//!    `call` and *waits for the other side's offer* as the next step of the
//!    same call; that offer arrives through the same `handle_offer` path as
//!    a fresh incoming call. The check is the slot already being `Claimed`
//!    with this exact `pairing_id`/`call_id` — then apply immediately
//!    (`ApplyRemoteOffer`), no ring, no countdown. Without it a caller sees
//!    its own outgoing call reflected back as a bogus "Incoming call from X".
//!    It only reproduces in *one* tie-break direction, so both are tests:
//!    `handle_offer_applies_immediately_when_it_completes_a_call_this_side_already_placed_tiebreak_direction_a`
//!    and `..._direction_b`.
//! 4. **Ending a call reads the pairing/call id before tearing anything
//!    down.** `hang_up` captures both with one `std::mem::replace` that
//!    clears the slot in the same statement — structurally atomic — then
//!    builds the `SendBye` from the captured values. The bug class (reading
//!    state after clearing it) can't be reintroduced in a shell: there is one
//!    `hang_up`. Tests: `hang_up_sends_bye_using_the_captured_pairing_and_call_id`,
//!    `hang_up_clears_state_so_a_second_hang_up_sends_no_bye`.
//! 5. **A media/capture failure only means "end the call" if a call is
//!    actually in progress.** `should_end_call_on_media_failure` is trivial
//!    today (the input echoed back) but kept as one place to route through:
//!    ringing acquires the camera before anyone has decided anything, so it
//!    must not be as fragile to a camera failure as an active call is. Web
//!    has no camera-failure trigger wired to it yet — a known, accepted gap.
//!    Test: `should_end_call_on_media_failure_matches_has_active_peer_connection`.
//! 6. **Every teardown path clears the incoming-call state, not just the
//!    slot.** `hang_up`/`handle_peer_hangup`/`handle_peer_left`/
//!    `forget_pairing` all set the slot to `Idle` (one enum, so that clears a
//!    live ring in the same motion) and emit `ClearIncomingCallTimer`. A
//!    stale countdown tick firing after the call resolved some other way is
//!    the bug this guards; `tick_incoming_call_countdown` also re-checks the
//!    slot is still `Ringing` for this exact id, but that's a second line of
//!    defense, not a substitute for clearing eagerly. Tests:
//!    `handle_peer_hangup_clears_a_pending_ring_too`,
//!    `forget_pairing_closes_the_active_call_without_sending_bye`,
//!    `tick_incoming_call_countdown_is_stale_after_accept`.
//! 7. **Redelivery of the same offer/call while already ringing is a no-op,
//!    not a reset.** Public relays redeliver ephemeral events under load; a
//!    redelivered offer must not restart an auto-answer countdown from 5 or
//!    re-ring a contact already ringing. `handle_offer` guards on the slot
//!    being `Claimed` with its offer applied, and on being `Ringing` for the
//!    same ids; `handle_should_offer` on the pairing already owning the
//!    slot. Tests: `handle_offer_redelivered_while_ringing_is_a_no_op`,
//!    `handle_should_offer_is_a_no_op_when_redelivered_for_the_same_call`,
//!    `handle_should_offer_is_a_no_op_for_a_second_call_id_while_already_active`,
//!    `handle_offer_is_a_no_op_when_already_active_with_a_real_peer_connection`,
//!    `handle_offer_redelivered_after_accept_before_pc_exists_is_a_no_op`.
//! 8. **The pubkey tie-break is the *first* thing `handle_should_offer`
//!    checks, before busy/redelivery** — a `call` from the side that should
//!    have lost is a silent no-op. Every other message type is forwarded
//!    into this crate unconditionally with Rust deciding each no-op; `call`
//!    gets the same treatment. Test:
//!    `handle_should_offer_no_ops_when_this_side_would_lose_the_tiebreak`.
//! 9. **A `busy` reply releases the *caller's own* slot**, not just a
//!    contact-list badge — otherwise a caller bounced by busy is stuck on the
//!    calling screen, unable to call anyone. `handle_peer_busy` is gated like
//!    `handle_peer_hangup` (both `pairing_id` and `call_id` must match the
//!    active attempt). Tests:
//!    `handle_peer_busy_releases_the_callers_slot_when_pairing_and_call_id_match`,
//!    `handle_peer_busy_requires_both_pairing_and_call_id_to_match`.
//!
//! # Changing this module
//!
//! - Change the Rust and add a test; don't "port to both platforms". What
//!   genuinely stays per-platform: the `answerApplied`/real-`signalingState`
//!   second guard in `WebRtcEngine.kt`'s `handleRemoteAnswer` and `app.js`'s
//!   `onAnswer` (a deliberate second guard on WebRTC state this module can't
//!   see); wake-lock/foreground-service lifecycle (Android only); and the
//!   shells' timer-loop scheduling mechanics (intentionally different, no
//!   observable consequence).
//! - Anything that runs *before* a call is accepted (a preview, a background
//!   acquisition) must not be able to kill the slot or a pending offer on
//!   failure — see #5.
//! - A new way for a call to end must go through the shared teardown path
//!   (#6), and must capture the ids before clearing (#4).
//! - Touching `request_call`, `handle_peer_online`, `handle_should_offer` or
//!   `wins_tiebreak`: add tests for **both** tie-break directions — one
//!   can't prove the other (#3). A live two-peer test is still worth doing
//!   for anything touching real WebRTC negotiation, though the tie-break
//!   *decision* is fully covered by `cargo test`.

use serde::Serialize;
#[cfg(test)]
use std::sync::Mutex;

/// Mirrors `CameraAgentService.AUTO_ANSWER_COUNTDOWN_SECONDS`/`app.js`'s
/// constant of the same name.
const AUTO_ANSWER_COUNTDOWN_SECONDS: u32 = 5;

/// How long a claimed-but-never-connected call slot can sit before
/// [`check_call_timeout`] gives up on it — see that function's own doc for
/// why this exists at all. Chosen to land in the same range as traditional
/// phone-ring timeouts (long enough that a slow network or a slow human
/// isn't punished, short enough that a lost message doesn't strand someone
/// on "Calling…" indefinitely).
pub(crate) const CALL_ANSWER_TIMEOUT_MS: i64 = 60_000;

/// The one call this device is currently placing, ringing for, or
/// negotiating. The payload of [`CallSlot::Claimed`].
struct ActiveCall {
    pairing_id: String,
    call_id: String,
    /// Guards against a redelivered `answer` being applied twice. The real
    /// WebRTC `signalingState` check both platforms also have stays a
    /// second, independent guard in the shell.
    answer_applied: bool,
    /// True once this exact call's offer has been handed to the shell for
    /// real (either [`handle_offer`]'s tie-break fast-path, or
    /// [`accept_incoming_call`] taking `pending_offer`) — distinguishes an
    /// ordinary accepted call whose `PeerConnection` hasn't been built yet
    /// from a genuine tie-break completion, since both look identical to
    /// `has_active_peer_connection` alone.
    offer_applied: bool,
    /// True once this call's real PeerConnection has ever reached
    /// CONNECTED — set by [`mark_connected`], read to decide
    /// `CallOutcomeReason::NeverConnected` vs `::Dropped`.
    connected_once: bool,
    /// When this slot was (most recently) claimed — refreshed every time
    /// this pairing's `ActiveCall` is (re)constructed, including
    /// [`handle_peer_online`] resolving a deferred call, so a peer who was
    /// briefly offline gets a fresh full window once a real message
    /// actually goes out to them, not whatever was left of the original
    /// deferred-since clock. Read by [`check_call_timeout`].
    claimed_at_ms: i64,
}

/// A call [`request_call`] wanted to place but couldn't yet — the peer
/// wasn't online. Resolved by [`handle_peer_online`] once presence notices
/// the peer is online. A single `Option`, not a map: [`request_call`]'s own
/// slot-ownership guard means at most one pairing can ever be deferred at a
/// time.
struct DeferredCall {
    pairing_id: String,
    call_id: String,
}

/// The device-wide call slot — exactly one of three states at any moment.
/// Used to be two separate `Option`s (`active`/`pending_offer`) that could
/// in principle disagree; collapsed into one enum once tracing every real
/// call site confirmed nothing needed them to disagree.
enum CallSlot {
    Idle,
    /// See [`PendingOffer`]'s own doc.
    Ringing(PendingOffer),
    /// See [`ActiveCall`]'s own doc.
    Claimed(ActiveCall),
}

impl CallSlot {
    fn pairing_id(&self) -> Option<&str> {
        match self {
            CallSlot::Idle => None,
            CallSlot::Ringing(pending) => Some(&pending.pairing_id),
            CallSlot::Claimed(active) => Some(&active.pairing_id),
        }
    }

    fn call_id(&self) -> Option<&str> {
        match self {
            CallSlot::Idle => None,
            CallSlot::Ringing(pending) => Some(&pending.call_id),
            CallSlot::Claimed(active) => Some(&active.call_id),
        }
    }
}

/// This module's whole state — a field of `crate::AppState`, not its own
/// separately-locked static.
pub(crate) struct CallState {
    slot: CallSlot,
    deferred_call: Option<DeferredCall>,
    /// Set by [`note_media_failure`] for the call it was reported against;
    /// read by [`peer_connection_closed`] so the outcome says "camera", not
    /// "network". Only counts while that same call still holds the slot.
    media_failed: Option<(String, String)>,
}

impl CallState {
    pub(crate) fn new() -> Self {
        CallState { slot: CallSlot::Idle, deferred_call: None, media_failed: None }
    }
}

/// An offer that's arrived but hasn't been handed to the real
/// `PeerConnection` yet — either auto-answer's countdown hasn't elapsed, or
/// this contact needs a manual Accept tap (see [`handle_offer`]).
/// `ice_buffer` holds ICE candidates that arrive during that window
/// (trickle ICE means the caller can be sending them before ringing even
/// resolves).
struct PendingOffer {
    pairing_id: String,
    call_id: String,
    kind: PendingOfferKind,
    ice_buffer: Vec<IceCandidate>,
    seconds_remaining: u32,
}

/// What actually happens once this ring is accepted — see
/// [`accept_incoming_call`]. Two variants, not a bare `sdp: String`: "I
/// already have the peer's SDP" (`RemoteOffer`) is a genuinely different
/// situation from "nothing negotiated yet, this side must create the offer"
/// (`NeedsOwnOffer`) — conflating them used to skip ringing entirely for
/// one of them. Both go through the same ring/auto-answer/accept path via
/// [`start_ringing`], diverging only in what [`accept_incoming_call`] does.
enum PendingOfferKind {
    /// The peer already sent a real offer — accepting means applying it
    /// ([`CallEffect::ApplyRemoteOffer`], handled by [`handle_offer`]'s
    /// fresh-call branch).
    RemoteOffer(String),
    /// A `"call"` message's recipient won the pubkey tie-break
    /// ([`handle_should_offer`]) — nothing has been negotiated yet;
    /// accepting means *this* side creates and sends the offer
    /// ([`CallEffect::CreateOffer`]).
    NeedsOwnOffer,
}

#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct IceCandidate {
    pub sdp_mid: Option<String>,
    pub sdp_m_line_index: i32,
    pub candidate: String,
}

/// Same shape as both platforms' `randomCallId()` (4 random bytes, hex
/// encoded) — a correlation id, not a secret.
fn random_call_id() -> String {
    let mut buf = [0u8; 4];
    getrandom::getrandom(&mut buf).expect("OS RNG must be available");
    crate::hex_encode(&buf)
}

/// Effects the shell must actually perform — WebRTC lifecycle and signaling
/// sends. Every variant carries only plain data (never a `pc` handle, since
/// `pc` never exists inside this module); the shell owns all the real I/O.
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum CallEffect {
    AcquireMedia,
    CreateOffer { pairing_id: String, call_id: String },
    SendCall { pairing_id: String, call_id: String },
    SendBusy { pairing_id: String, call_id: String },
    ApplyRemoteOffer { pairing_id: String, call_id: String, sdp: String },
    StartRinging { pairing_id: String, call_id: String, auto_answer: bool, seconds_remaining: u32 },
    SendBye { pairing_id: String, call_id: String },
    /// A terminal call needs a full-screen message shown to the user —
    /// distinct from a plain [`ClosePeerConnection`](CallEffect::ClosePeerConnection),
    /// which fires unconditionally on every teardown including a local hang
    /// up (which needs no explanation). See [`CallOutcomeReason`] for which
    /// teardown paths emit this.
    ShowCallOutcome { pairing_id: String, call_id: String, reason: CallOutcomeReason },
    ClosePeerConnection,
    ClearIncomingCallTimer,
    /// Send a heartbeat now, outside the shell's schedule — emitted when a
    /// call is placed to a peer we believe is offline, together with a
    /// request that this heartbeat carry `hello` (see
    /// [`crate::presence::mark_seen`]): if the peer is actually online and we
    /// merely haven't heard from it (a reload, a missed heartbeat), it answers
    /// within a round trip and the call proceeds; if it really is offline, its
    /// own hello on coming back resolves the call.
    KickHeartbeat,
}

/// Why a call ended, for [`CallEffect::ShowCallOutcome`] — not every
/// teardown path emits this (`hang_up`/`forget_pairing` never do). Which
/// reason applies comes from what the slot looked like at that moment
/// (ringing or calling, whether an offer/answer was exchanged,
/// `ActiveCall::connected_once`) and which signal arrived.
#[derive(Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcomeReason {
    /// The other side hung up a call that was already underway.
    PeerEnded,
    /// We were calling and they said no before anything was negotiated.
    Declined,
    /// We were ringing and they hung up before we answered (a missed call).
    Cancelled,
    /// We called and nothing ever answered, though the peer looked reachable.
    NoAnswer,
    /// We called and the peer isn't reachable (offline, or went offline).
    Unreachable,
    /// The peer is on another call.
    Busy,
    /// This device's own camera or microphone failed.
    CameraFailed,
    /// Both sides were there but the media never connected.
    NeverConnected,
    /// Was connected, then lost.
    Dropped,
}

/// Claims the call slot for `pairing_id`/`call_id` and tells the shell to
/// create an offer immediately, no ring — shared by the winning half of
/// [`request_call`]'s/[`handle_peer_online`]'s pubkey tie-break, where a
/// human already tapped Call so there's nothing left to ring for. **Not**
/// used by [`handle_should_offer`] — that path goes through
/// [`start_ringing`] instead; see [`PendingOfferKind`]'s doc for why.
fn claim_slot_and_create_offer(state: &mut CallState, pairing_id: &str, call_id: &str, now_ms: i64) -> Vec<CallEffect> {
    state.slot = CallSlot::Claimed(ActiveCall {
        pairing_id: pairing_id.to_string(),
        call_id: call_id.to_string(),
        answer_applied: false,
        offer_applied: false,
        connected_once: false,
        claimed_at_ms: now_ms,
    });
    vec![CallEffect::CreateOffer { pairing_id: pairing_id.to_string(), call_id: call_id.to_string() }]
}

/// Claims the call slot and starts a proper ring — shared by
/// [`handle_offer`]'s fresh-call branch (`kind: RemoteOffer`) and
/// [`handle_should_offer`]'s tie-break-win branch (`kind: NeedsOwnOffer`).
/// Both are equally "an incoming call a human needs to know about";
/// [`accept_incoming_call`] is the only place they diverge.
fn start_ringing(state: &mut CallState, pairing_id: &str, call_id: &str, kind: PendingOfferKind, auto_answer: bool) -> Vec<CallEffect> {
    state.slot = CallSlot::Ringing(PendingOffer {
        pairing_id: pairing_id.to_string(),
        call_id: call_id.to_string(),
        kind,
        ice_buffer: Vec::new(),
        seconds_remaining: AUTO_ANSWER_COUNTDOWN_SECONDS,
    });
    vec![
        CallEffect::AcquireMedia,
        CallEffect::StartRinging {
            pairing_id: pairing_id.to_string(),
            call_id: call_id.to_string(),
            auto_answer,
            seconds_remaining: AUTO_ANSWER_COUNTDOWN_SECONDS,
        },
    ]
}

/// What starting a call attempt hands back: the `call_id` assigned (`None`
/// only when a *different* pairing already owns the slot — a silent no-op,
/// the UI already prevents this path), and whatever effects follow.
#[derive(Serialize, Debug, PartialEq)]
pub struct RequestCallResult {
    pub call_id: Option<String>,
    pub effects: Vec<CallEffect>,
}

/// The pubkey tie-break used throughout this module: given two confirmed
/// peers' pubkeys, the lexicographically lower one always offers.
fn wins_tiebreak(own_pubkey_hex: &str, peer_pubkey_hex: &str) -> bool {
    own_pubkey_hex < peer_pubkey_hex
}

/// Starts a call attempt — mirrors `CameraAgentService.requestCall` +
/// `NostrSignalingClient.requestCall`'s pubkey tie-break, unified into one
/// call. `peer_online` is supplied by the shell's own presence tracking;
/// this function only decides what happens once that fact is known.
pub fn request_call(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, peer_online: bool, now_ms: i64) -> RequestCallResult {
    let mut guard = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let app = &mut *guard;
    let state = &mut app.call;
    if let Some(active) = state.slot.pairing_id() {
        if active != pairing_id {
            return RequestCallResult { call_id: None, effects: vec![] };
        }
    }
    let call_id = random_call_id();

    if !peer_online {
        app.presence.request_hello();
        state.deferred_call = Some(DeferredCall { pairing_id: pairing_id.to_string(), call_id: call_id.clone() });
        state.slot = CallSlot::Claimed(ActiveCall {
            pairing_id: pairing_id.to_string(),
            call_id: call_id.clone(),
            answer_applied: false,
            offer_applied: false,
            connected_once: false,
            claimed_at_ms: now_ms,
        });
        return RequestCallResult { call_id: Some(call_id), effects: vec![CallEffect::AcquireMedia, CallEffect::KickHeartbeat] };
    }

    if wins_tiebreak(own_pubkey_hex, peer_pubkey_hex) {
        let mut effects = vec![CallEffect::AcquireMedia];
        effects.extend(claim_slot_and_create_offer(state, pairing_id, &call_id, now_ms));
        return RequestCallResult { call_id: Some(call_id), effects };
    }

    state.slot = CallSlot::Claimed(ActiveCall {
        pairing_id: pairing_id.to_string(),
        call_id: call_id.clone(),
        answer_applied: false,
        offer_applied: false,
        connected_once: false,
        claimed_at_ms: now_ms,
    });
    RequestCallResult {
        call_id: Some(call_id.clone()),
        effects: vec![CallEffect::AcquireMedia, CallEffect::SendCall { pairing_id: pairing_id.to_string(), call_id }],
    }
}

/// Called by the shell's presence layer the moment a peer transitions
/// online — resolves any `DeferredCall` waiting on this pairing. `[]` if
/// none was actually wanted.
///
/// Locking wrapper around `inner_handle_peer_online` for `android.rs`/
/// `wasm.rs`; `presence` itself calls `inner_handle_peer_online` directly
/// since it's already holding `crate::STATE`'s guard.
pub fn handle_peer_online(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, now_ms: i64) -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    inner_handle_peer_online(&mut app.call, pairing_id, own_pubkey_hex, peer_pubkey_hex, now_ms)
}

pub(crate) fn inner_handle_peer_online(state: &mut CallState, pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, now_ms: i64) -> Vec<CallEffect> {
    if !state.deferred_call.as_ref().is_some_and(|d| d.pairing_id == pairing_id) {
        return vec![];
    }
    let call_id = state.deferred_call.take().unwrap().call_id;

    if wins_tiebreak(own_pubkey_hex, peer_pubkey_hex) {
        claim_slot_and_create_offer(state, pairing_id, &call_id, now_ms)
    } else {
        state.slot = CallSlot::Claimed(ActiveCall {
            pairing_id: pairing_id.to_string(),
            call_id: call_id.clone(),
            answer_applied: false,
            offer_applied: false,
            connected_once: false,
            claimed_at_ms: now_ms,
        });
        vec![CallEffect::SendCall { pairing_id: pairing_id.to_string(), call_id }]
    }
}

/// Mirrors `onShouldOffer`/its web twin: **the pubkey tie-break first** (a
/// `"call"` message only reaches this function if this side wins it — a
/// silent no-op otherwise), then busy guard (a different pairing already
/// owns the slot), then redelivery guard (this pairing already owns the
/// slot), else claims the slot and **rings**, same as a fresh
/// [`handle_offer`] call — this side does not create the offer immediately
/// even though it won the tie-break, since a human still needs to see and
/// accept the ring (see `PendingOfferKind`'s doc for the bug this fixes).
///
/// Deliberately does not gate the redelivery guard on
/// `has_active_peer_connection`: `state.slot`'s own `pairing_id()` match is
/// already sufficient, since it's set synchronously the instant this side's
/// own call claims the slot, with no async gap to race.
pub fn handle_should_offer(pairing_id: &str, call_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, auto_answer: bool) -> Vec<CallEffect> {
    if !wins_tiebreak(own_pubkey_hex, peer_pubkey_hex) {
        return vec![];
    }
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    if let Some(active) = state.slot.pairing_id() {
        if active != pairing_id {
            return vec![CallEffect::SendBusy { pairing_id: pairing_id.to_string(), call_id: call_id.to_string() }];
        }
    }
    if state.slot.pairing_id() == Some(pairing_id) {
        return vec![];
    }
    start_ringing(state, pairing_id, call_id, PendingOfferKind::NeedsOwnOffer, auto_answer)
}

/// Mirrors `onOffer`/`onOfferReceived`: busy guard, already-active
/// redelivery guard, already-ringing redelivery guard, then the
/// pubkey-tie-break fast-path (module-doc invariant #3: an offer that
/// completes a call *this side* already placed applies immediately, no
/// ring). Otherwise claims the slot, stores the pending offer, and tells
/// the shell to acquire media + start ringing. `auto_answer` is looked up
/// by the shell (its own contacts list) and passed in.
///
/// Redelivery is gated on `offer_applied` alone, not also
/// `has_active_peer_connection`: `offer_applied` is set synchronously the
/// moment an offer is first handed to the shell, strictly before the
/// shell's own async `PeerConnection` construction could ever finish — so
/// the two conditions can never actually disagree.
pub fn handle_offer(pairing_id: &str, call_id: &str, sdp: &str, auto_answer: bool) -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;

    if let Some(active) = state.slot.pairing_id() {
        if active != pairing_id {
            return vec![CallEffect::SendBusy { pairing_id: pairing_id.to_string(), call_id: call_id.to_string() }];
        }
    }
    // A redelivered offer for a call already handed to the shell is a
    // no-op; ringing's own redelivery guard is folded into the same match.
    match &mut state.slot {
        CallSlot::Claimed(active) if active.pairing_id == pairing_id && active.call_id == call_id => {
            if active.offer_applied {
                return vec![];
            }
            active.offer_applied = true;
            return vec![CallEffect::ApplyRemoteOffer { pairing_id: pairing_id.to_string(), call_id: call_id.to_string(), sdp: sdp.to_string() }];
        }
        CallSlot::Ringing(pending) if pending.pairing_id == pairing_id && pending.call_id == call_id => {
            return vec![];
        }
        _ => {}
    }

    start_ringing(state, pairing_id, call_id, PendingOfferKind::RemoteOffer(sdp.to_string()), auto_answer)
}

/// Mirrors `onAnswer`/`handleAnswer`'s `activePairingId`/`activeCallId`/
/// `answerApplied` check — sets the internal `answer_applied` flag when
/// returning `true`. Deliberately excludes the real WebRTC `signalingState
/// !== 'have-local-offer'` check both platforms also have — that's
/// `pc`-internal state with no equivalent here, and stays a second,
/// independent guard inside `WebRtcEngine`/`ensurePeerConnection`.
///
/// Only ever `true` while `CallSlot::Claimed`, never
/// `CallSlot::Ringing` — a real answer can only be a reply to an offer
/// this side already sent, and this side only ever sends one from
/// `Claimed`, never mid-ring.
pub fn should_apply_answer(pairing_id: &str, call_id: &str) -> bool {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    let CallSlot::Claimed(active) = &mut state.slot else { return false };
    if active.pairing_id != pairing_id || active.call_id != call_id || active.answer_applied {
        return false;
    }
    active.answer_applied = true;
    true
}

/// Called by the shell the instant its real PeerConnection reaches
/// CONNECTED — mirrors [`should_apply_answer`]'s pairing_id+call_id guard
/// so a delayed native callback can't mark a different, already-superseded
/// call as connected. A no-op, not a panic, when the ids don't match.
pub fn mark_connected(pairing_id: &str, call_id: &str) {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    if let CallSlot::Claimed(active) = &mut state.slot {
        if active.pairing_id == pairing_id && active.call_id == call_id {
            active.connected_once = true;
        }
    }
}

/// What to do with an incoming ICE candidate.
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "outcome")]
pub enum IceOutcome {
    /// Buffered into the still-ringing `pending_offer` — trickle ICE means
    /// candidates can arrive before the offer is even accepted.
    Buffered,
    /// Matches the active call — apply it to the real `PeerConnection` now.
    Apply { sdp_mid: Option<String>, sdp_m_line_index: i32, candidate: String },
    /// Matches neither the pending ring nor the active call — drop
    /// silently (matches both platforms' fallthrough behavior).
    Dropped,
}

pub fn handle_remote_ice(pairing_id: &str, call_id: &str, sdp_mid: Option<&str>, sdp_m_line_index: i32, candidate: &str) -> IceOutcome {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    match &mut state.slot {
        CallSlot::Ringing(pending) if pending.pairing_id == pairing_id && pending.call_id == call_id => {
            pending.ice_buffer.push(IceCandidate { sdp_mid: sdp_mid.map(String::from), sdp_m_line_index, candidate: candidate.to_string() });
            IceOutcome::Buffered
        }
        CallSlot::Claimed(active) if active.pairing_id == pairing_id && active.call_id == call_id => {
            IceOutcome::Apply { sdp_mid: sdp_mid.map(String::from), sdp_m_line_index, candidate: candidate.to_string() }
        }
        _ => IceOutcome::Dropped,
    }
}

/// Everything the shell needs to actually apply an accepted incoming call —
/// mirrors `acceptIncomingCall`: takes `pending_offer` if present (clearing
/// it and its countdown-timer state), and tells the shell which of the two
/// things `PendingOfferKind` describes it needs to do now.
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum AcceptOutcome {
    /// The peer's offer was already in hand — apply it now.
    ApplyOffer { pairing_id: String, call_id: String, sdp: String, ice_buffer: Vec<IceCandidate> },
    /// Nothing negotiated yet (this side won the tie-break on a `"call"`
    /// message) — create and send the offer now.
    CreateOffer { pairing_id: String, call_id: String },
}

/// `None` if there's nothing pending — a no-op (e.g. a stray UI tap after
/// the call already resolved some other way).
pub fn accept_incoming_call(now_ms: i64) -> Option<AcceptOutcome> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    // Swap in Idle unconditionally, then put back whatever wasn't Ringing —
    // the standard "take conditionally" shape for an enum with no cheap way
    // to peek-and-take one variant in place.
    let pending = match std::mem::replace(&mut state.slot, CallSlot::Idle) {
        CallSlot::Ringing(pending) => pending,
        other => {
            state.slot = other;
            return None;
        }
    };
    // offer_applied: true here too — this is the other place besides
    // handle_offer's tie-break fast-path an offer gets handed to the shell
    // for real; see ActiveCall::offer_applied's own doc.
    state.slot = CallSlot::Claimed(ActiveCall {
        pairing_id: pending.pairing_id.clone(),
        call_id: pending.call_id.clone(),
        answer_applied: false,
        offer_applied: true,
        connected_once: false,
        claimed_at_ms: now_ms,
    });
    Some(match pending.kind {
        PendingOfferKind::RemoteOffer(sdp) => {
            AcceptOutcome::ApplyOffer { pairing_id: pending.pairing_id, call_id: pending.call_id, sdp, ice_buffer: pending.ice_buffer }
        }
        PendingOfferKind::NeedsOwnOffer => AcceptOutcome::CreateOffer { pairing_id: pending.pairing_id, call_id: pending.call_id },
    })
}

/// One tick of the auto-answer countdown — the *decision* only; the shell
/// keeps owning the actual timer loop (a 1-second reschedule on
/// `Continue`), the same way pairing-bootstrap's `handle_timeout` already
/// works (core decides, shell schedules). The shell calls this once per
/// second starting one second *after* [`handle_offer`] returned
/// `StartRinging` with `seconds_remaining: 5` — never a synchronous
/// zeroth call.
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "outcome")]
pub enum TickOutcome {
    /// `pending_offer` no longer matches `pairing_id`/`call_id` — the call
    /// was already accepted, declined, or superseded by a fresh ring.
    Stale,
    Continue { seconds_remaining: u32 },
    /// The shell should call [`accept_incoming_call`] now.
    ShouldAccept,
}

pub fn tick_incoming_call_countdown(pairing_id: &str, call_id: &str) -> TickOutcome {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    let CallSlot::Ringing(pending) = &mut state.slot else {
        return TickOutcome::Stale;
    };
    if pending.pairing_id != pairing_id || pending.call_id != call_id {
        return TickOutcome::Stale;
    }
    if pending.seconds_remaining <= 1 {
        return TickOutcome::ShouldAccept;
    }
    pending.seconds_remaining -= 1;
    TickOutcome::Continue { seconds_remaining: pending.seconds_remaining }
}

/// **The module-doc invariant #4 function.** Captures `pairing_id`/
/// `call_id` before clearing anything, so `SendBye` is built from the
/// captured values, never re-read after the slot is already cleared.
/// Clears `pending_offer`/timer state and any deferred-call bookkeeping.
/// Always emits `ClearIncomingCallTimer` + `ClosePeerConnection`; `SendBye`
/// only if a pairing was actually active. Never emits
/// [`CallEffect::ShowCallOutcome`] — this is the self-initiated path; the
/// person hanging up already knows why.
pub fn hang_up() -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    let (pairing_id, call_id) = match std::mem::replace(&mut state.slot, CallSlot::Idle) {
        CallSlot::Idle => (None, None),
        CallSlot::Ringing(pending) => (Some(pending.pairing_id), Some(pending.call_id)),
        CallSlot::Claimed(active) => (Some(active.pairing_id), Some(active.call_id)),
    };
    if let Some(pid) = &pairing_id {
        if state.deferred_call.as_ref().is_some_and(|d| &d.pairing_id == pid) {
            state.deferred_call = None;
        }
    }

    let mut effects = vec![CallEffect::ClearIncomingCallTimer];
    if let Some(pairing_id) = pairing_id {
        let call_id = call_id.unwrap_or_else(random_call_id);
        effects.push(CallEffect::SendBye { pairing_id, call_id });
    }
    effects.push(CallEffect::ClosePeerConnection);
    effects
}

/// Polled by the shell on its own existing tick (mirrors
/// [`crate::presence::check_online_timeouts`]'s shape exactly — a pure,
/// periodically-polled function rather than an internal timer, since this
/// module has no clock of its own) — gives up on a call that's been
/// `CallSlot::Claimed` too long without ever reaching
/// [`mark_connected`]. Covers every way a `Claimed` slot can be waiting on
/// the *other* side's very first response with nothing left to fail from
/// on its own: a sent offer with no answer yet, a sent `"call"` ping with
/// no reply, or a deferred call whose peer never came online — `request_call`
/// sets `Claimed` (and `claimed_at_ms`) synchronously in all three cases, so
/// one check covers all three. Deliberately does **not** apply to
/// `CallSlot::Ringing`: an incoming ring is bounded by a human (Accept/
/// Decline are always on screen) or by auto-answer's own short countdown,
/// never by this.
///
/// On timeout: exactly [`hang_up`]'s own effects (`SendBye` covers the case
/// where the peer *did* get the offer and is sitting there ringing; the
/// timed-out side may as well tell them) plus
/// [`CallEffect::ShowCallOutcome`] — the one thing `hang_up` deliberately
/// never emits, since here the person doesn't already know why: `Unreachable`
/// for a peer that never came online, `NoAnswer` for one that was there but
/// never answered, `NeverConnected` once an offer/answer was exchanged.
pub fn check_call_timeout(now_ms: i64) -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    let expired = matches!(
        &state.slot,
        CallSlot::Claimed(active) if !active.connected_once && now_ms - active.claimed_at_ms >= CALL_ANSWER_TIMEOUT_MS
    );
    if !expired {
        return vec![];
    }
    let CallSlot::Claimed(active) = std::mem::replace(&mut state.slot, CallSlot::Idle) else { unreachable!() };
    let was_deferred = state.deferred_call.as_ref().is_some_and(|d| d.pairing_id == active.pairing_id);
    if was_deferred {
        state.deferred_call = None;
    }
    // Nothing ever came back from a peer that never showed up online:
    // unreachable. Nothing came back from one that did: no answer. An
    // offer or answer did get exchanged: the media is what failed.
    let reason = if was_deferred {
        CallOutcomeReason::Unreachable
    } else if active.answer_applied || active.offer_applied {
        CallOutcomeReason::NeverConnected
    } else {
        CallOutcomeReason::NoAnswer
    };
    vec![
        CallEffect::ClearIncomingCallTimer,
        CallEffect::SendBye { pairing_id: active.pairing_id.clone(), call_id: active.call_id.clone() },
        CallEffect::ClosePeerConnection,
        CallEffect::ShowCallOutcome { pairing_id: active.pairing_id, call_id: active.call_id, reason },
    ]
}

/// Shared by [`handle_peer_hangup`] and [`handle_peer_busy`] — same guard,
/// same cleared fields. Returns `false` when `pairing_id`/`call_id` don't
/// match the active call — a message about an attempt this device has
/// already moved on from must not tear down a different, later one.
fn end_active_call_if_matching(state: &mut CallState, pairing_id: &str, call_id: &str) -> bool {
    if state.slot.pairing_id() != Some(pairing_id) || state.slot.call_id() != Some(call_id) {
        return false;
    }
    state.slot = CallSlot::Idle;
    true
}

/// Mirrors `onPeerHangup` — gated on **both** `pairing_id` and `call_id`
/// matching. Same effects as [`hang_up`] minus `SendBye` (we don't bye a
/// bye) plus [`CallEffect::ShowCallOutcome`]: `Cancelled` if we were still
/// ringing, `Declined` if we were calling and nothing had been exchanged,
/// otherwise `PeerEnded` — an explicit "bye" is the most specific signal
/// available, reported regardless of `ActiveCall::connected_once`.
pub fn handle_peer_hangup(pairing_id: &str, call_id: &str) -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    // Read before the slot is cleared: what a "bye" means depends on what
    // it ended.
    let reason = match &state.slot {
        CallSlot::Ringing(p) if p.pairing_id == pairing_id && p.call_id == call_id => CallOutcomeReason::Cancelled,
        CallSlot::Claimed(a) if a.pairing_id == pairing_id && a.call_id == call_id && !a.connected_once && !a.answer_applied && !a.offer_applied => {
            CallOutcomeReason::Declined
        }
        _ => CallOutcomeReason::PeerEnded,
    };
    if !end_active_call_if_matching(state, pairing_id, call_id) {
        return vec![];
    }
    vec![
        CallEffect::ClearIncomingCallTimer,
        CallEffect::ShowCallOutcome { pairing_id: pairing_id.to_string(), call_id: call_id.to_string(), reason },
        CallEffect::ClosePeerConnection,
    ]
}

/// Mirrors receiving a `"busy"` reply to our own outgoing call attempt —
/// releases the caller's own claimed slot (without this, the caller would
/// be stuck on the calling screen with no way back to idle) and says why
/// with a `Busy` outcome. Same gating as [`handle_peer_hangup`].
pub fn handle_peer_busy(pairing_id: &str, call_id: &str) -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    inner_handle_peer_busy(&mut app.call, pairing_id, call_id)
}

pub(crate) fn inner_handle_peer_busy(state: &mut CallState, pairing_id: &str, call_id: &str) -> Vec<CallEffect> {
    if !end_active_call_if_matching(state, pairing_id, call_id) {
        return vec![];
    }
    vec![
        CallEffect::ClearIncomingCallTimer,
        CallEffect::ShowCallOutcome { pairing_id: pairing_id.to_string(), call_id: call_id.to_string(), reason: CallOutcomeReason::Busy },
        CallEffect::ClosePeerConnection,
    ]
}

/// Mirrors `onPeerLeft` — **no `call_id` check, unlike `bye`'s
/// call_id-scoped guard**: a peer going fully offline ends *any* call with
/// them regardless of which `call_id`. Confirmed intentional on both
/// platforms, not a bug to fix. Reports `Dropped` for a call that had
/// connected, `Cancelled` for one still ringing, and `Unreachable` for one we
/// were still placing — the best signal available for a peer that vanished
/// from presence without an explicit "bye".
pub fn handle_peer_left(pairing_id: &str) -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    inner_handle_peer_left(&mut app.call, pairing_id)
}

pub(crate) fn inner_handle_peer_left(state: &mut CallState, pairing_id: &str) -> Vec<CallEffect> {
    if state.slot.pairing_id() != Some(pairing_id) {
        return vec![];
    }
    let call_id = state.slot.call_id().unwrap_or_default().to_string();
    let reason = match &state.slot {
        CallSlot::Ringing(_) => CallOutcomeReason::Cancelled,
        CallSlot::Claimed(active) if active.connected_once => CallOutcomeReason::Dropped,
        _ => CallOutcomeReason::Unreachable,
    };
    state.slot = CallSlot::Idle;
    vec![
        CallEffect::ClearIncomingCallTimer,
        CallEffect::ShowCallOutcome { pairing_id: pairing_id.to_string(), call_id, reason },
        CallEffect::ClosePeerConnection,
    ]
}

/// Mirrors `removePairing`'s "no bye sent on delete" behavior — same
/// cleanup as [`hang_up`] when `pairing_id` is the active call, but without
/// `SendBye` (a deleted contact isn't told anything) and without
/// [`CallEffect::ShowCallOutcome`]. Also clears `deferred_call` for
/// `pairing_id` unconditionally, even when it isn't the active call — a
/// deferred call waiting on presence must not fire for a contact that no
/// longer exists.
pub fn forget_pairing(pairing_id: &str) -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    if state.deferred_call.as_ref().is_some_and(|d| d.pairing_id == pairing_id) {
        state.deferred_call = None;
    }

    if state.slot.pairing_id() != Some(pairing_id) {
        return vec![];
    }

    state.slot = CallSlot::Idle;
    vec![CallEffect::ClearIncomingCallTimer, CallEffect::ClosePeerConnection]
}

/// The shell→core **event** (not a request/effect): call this any time the
/// shell notices a real `PeerConnection` is gone, for *any* reason —
/// including WebRTC's own spontaneous `FAILED`/`CLOSED` transition, or
/// `onMediaFailure` closing an active call. Idempotently clears the whole
/// slot, `pending_offer` included.
///
/// Emits [`CallEffect::ShowCallOutcome`] (`NeverConnected`/`Dropped`, same
/// `connected_once` heuristic as [`handle_peer_left`]) *only* when the slot
/// isn't already `CallSlot::Idle`. Every other teardown path already
/// clears the slot (and, where relevant, already emits its own outcome)
/// before its own `ClosePeerConnection` reaches the shell — so by the time
/// the shell's own teardown calls this function, it correctly no-ops unless
/// this really is a spontaneous drop with no prior call-core decision
/// behind it. That's what guarantees exactly one `ShowCallOutcome` per
/// call, never zero, never two.
///
/// Also emits [`CallEffect::SendBye`] in that same case — found live: a web
/// caller whose own `getUserMedia` was denied reached this function
/// directly (its *only* teardown step, `onMediaFailure`'s own call site, not
/// a follow-up after `hang_up`/`handle_peer_hangup` already ran), correctly
/// showed itself `NeverConnected`, but never told the peer anything at all —
/// the other side was left on "Incoming call"/"Calling…" indefinitely, since
/// `Ringing` has no timeout of its own (a human is meant to decide) and a
/// `Claimed` caller's own [`check_call_timeout`] could be minutes away.
/// Reaching this branch already means (by the reasoning above) no other
/// path sent a Bye yet, so this is genuinely the first and only chance to.
/// Safe even for the spontaneous-`FAILED` case this function also covers —
/// a redundant Bye the peer's own independent teardown made unnecessary is
/// just a harmless no-op on arrival ([`handle_peer_hangup`]'s own
/// pairing+call-id gate).
pub fn peer_connection_closed() -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    let media_failed = state.media_failed.take();
    let failed_here = |pairing_id: &str, call_id: &str| media_failed.as_ref().is_some_and(|(p, c)| p == pairing_id && c == call_id);
    match std::mem::replace(&mut state.slot, CallSlot::Idle) {
        CallSlot::Idle => vec![],
        CallSlot::Ringing(pending) => {
            let reason = if failed_here(&pending.pairing_id, &pending.call_id) { CallOutcomeReason::CameraFailed } else { CallOutcomeReason::NeverConnected };
            vec![
                CallEffect::SendBye { pairing_id: pending.pairing_id.clone(), call_id: pending.call_id.clone() },
                CallEffect::ShowCallOutcome { pairing_id: pending.pairing_id, call_id: pending.call_id, reason },
            ]
        }
        CallSlot::Claimed(active) => {
            let reason = if failed_here(&active.pairing_id, &active.call_id) {
                CallOutcomeReason::CameraFailed
            } else if active.connected_once {
                CallOutcomeReason::Dropped
            } else {
                CallOutcomeReason::NeverConnected
            };
            vec![
                CallEffect::SendBye { pairing_id: active.pairing_id.clone(), call_id: active.call_id.clone() },
                CallEffect::ShowCallOutcome { pairing_id: active.pairing_id, call_id: active.call_id, reason },
            ]
        }
    }
}

/// The shell calls this when this device's own camera or microphone failed
/// (a denied permission, a camera that errors or disconnects), *before* it
/// closes the connection: the outcome then says so instead of blaming the
/// network. Remembered for the call that holds the slot right now only; a
/// no-op when there is none.
pub fn note_media_failure() {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    state.media_failed = match (state.slot.pairing_id(), state.slot.call_id()) {
        (Some(p), Some(c)) => Some((p.to_string(), c.to_string())),
        _ => None,
    };
}

/// Mirrors `onMediaFailure`'s `if (pc != null)` guard: a live call
/// (`has_active_peer_connection`) ends outright on a camera/track failure;
/// a ringing-only preview (no `pc` yet) just releases the broken capture
/// and leaves the ring alone.
pub fn should_end_call_on_media_failure(has_active_peer_connection: bool) -> bool {
    has_active_peer_connection
}

/// Whether any pairing currently has a deferred call waiting on presence —
/// the shell's presence/heartbeat layer needs this to decide its adaptive
/// heartbeat rate.
pub fn any_call_wanted() -> bool {
    let app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    inner_any_call_wanted(&app.call)
}

pub(crate) fn inner_any_call_wanted(state: &CallState) -> bool {
    state.deferred_call.is_some()
}

/// Whether this device's own call slot is currently occupied — device-wide,
/// not per-pairing. Broadcast on every outgoing heartbeat as this device's
/// own busy status (see `presence::mark_seen`'s `peer_busy` parameter on
/// the receiving side).
pub fn is_call_active() -> bool {
    let app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    !matches!(app.call.slot, CallSlot::Idle)
}

/// Whether a contact's Call button is offered: no media is already connected
/// to it, and this device's call slot is free.
/// The last condition is belt-and-suspenders — placing a call to a different
/// contact while one is already active is a silent no-op in [`request_call`]
/// — but it also keeps the button from being offered at all, which is what
/// both shells' contact lists want. `connected`: the shell's own
/// per-contact "media is up" flag (a `PeerConnection` fact core never holds).
pub fn can_place_call(connected: bool) -> bool {
    !connected && !is_call_active()
}

/// Serializes call_arbitration tests (and, via [`reset_state_for_test`],
/// `presence`'s tests too, since `presence` calls directly into this
/// module) against each other — keeps `cargo test`'s parallel threads from
/// corrupting each other's runs on this shared state.
#[cfg(test)]
pub(crate) static TEST_SERIAL: Mutex<()> = Mutex::new(());

/// Resets just `crate::STATE`'s `call` field to a clean slate and returns
/// the serialization guard. Real call state is a single global slot, so
/// tests can't isolate from each other via unique ids alone. Recovers from
/// a poisoned lock rather than cascading a panic into every later test.
/// `pub(crate)` so `presence`'s own tests can reset this field too.
#[cfg(test)]
pub(crate) fn reset_state_for_test() -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).call = CallState::new();
    guard
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fresh_id() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        format!("test-call-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    // --- invariant #1: slot claimed immediately, not gated on acceptance ---

    #[test]
    fn request_call_claims_slot_and_offers_when_peer_online_and_own_pubkey_wins_tiebreak() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = request_call(&id, "aaa", "bbb", true, 0); // "aaa" < "bbb"
        assert!(result.call_id.is_some());
        assert!(matches!(result.effects.as_slice(), [CallEffect::AcquireMedia, CallEffect::CreateOffer { .. }]), "{:?}", result.effects);
    }

    #[test]
    fn request_call_claims_slot_and_sends_call_when_peer_online_and_own_pubkey_loses_tiebreak() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = request_call(&id, "bbb", "aaa", true, 0); // "bbb" > "aaa"
        assert!(result.call_id.is_some());
        assert!(matches!(result.effects.as_slice(), [CallEffect::AcquireMedia, CallEffect::SendCall { .. }]), "{:?}", result.effects);
    }

    #[test]
    fn request_call_defers_when_peer_offline_and_resolves_via_handle_peer_online_tiebreak_win() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = request_call(&id, "aaa", "bbb", false, 0);
        assert!(result.call_id.is_some());
        assert_eq!(result.effects, vec![CallEffect::AcquireMedia, CallEffect::KickHeartbeat]);
        let effects = handle_peer_online(&id, "aaa", "bbb", 0);
        assert!(matches!(effects.as_slice(), [CallEffect::CreateOffer { .. }]), "{:?}", effects);
    }

    #[test]
    fn request_call_defers_when_peer_offline_and_resolves_via_handle_peer_online_tiebreak_loss() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        request_call(&id, "bbb", "aaa", false, 0);
        let effects = handle_peer_online(&id, "bbb", "aaa", 0);
        assert!(matches!(effects.as_slice(), [CallEffect::SendCall { .. }]), "{:?}", effects);
    }

    #[test]
    fn handle_peer_online_is_a_no_op_when_nothing_was_deferred() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        assert!(handle_peer_online(&id, "aaa", "bbb", 0).is_empty());
    }

    #[test]
    fn request_call_no_ops_when_a_different_pairing_already_owns_the_slot() {
        let _guard = reset_state_for_test();
        let id_a = fresh_id();
        let id_b = fresh_id();
        request_call(&id_a, "aaa", "bbb", true, 0);
        let result = request_call(&id_b, "aaa", "bbb", true, 0);
        assert_eq!(result, RequestCallResult { call_id: None, effects: vec![] });
    }

    #[test]
    fn handle_should_offer_claims_slot_and_rings_respecting_auto_answer() {
        // See PendingOfferKind's own doc for the bug this guards against.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let effects = handle_should_offer(&id, "call1", "aaa", "bbb", true);
        assert!(
            matches!(effects.as_slice(), [CallEffect::AcquireMedia, CallEffect::StartRinging { auto_answer: true, seconds_remaining: 5, .. }]),
            "winning the tie-break on a 'call' message must ring like any other incoming call, not silently create an offer: {effects:?}"
        );
    }

    #[test]
    fn accept_incoming_call_needing_its_own_offer_hands_back_create_offer() {
        // See AcceptOutcome's own doc for why this and the sibling test
        // below need two variants, not one flat struct with an optional sdp.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        let outcome = accept_incoming_call(0).expect("a pending ring should exist");
        assert_eq!(outcome, AcceptOutcome::CreateOffer { pairing_id: id.clone(), call_id: "call1".to_string() });
        assert!(accept_incoming_call(0).is_none(), "accepting twice must be a no-op the second time");
    }

    // --- pubkey tie-break ---

    #[test]
    fn handle_should_offer_no_ops_when_this_side_would_lose_the_tiebreak() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        // "bbb" > "aaa": this side loses, so the message is silently ignored.
        let effects = handle_should_offer(&id, "call1", "bbb", "aaa", false);
        assert!(effects.is_empty(), "{effects:?}");
    }

    // --- invariant #7: redelivery guards ---

    #[test]
    fn handle_should_offer_sends_busy_when_a_different_pairing_owns_the_slot() {
        let _guard = reset_state_for_test();
        let id_a = fresh_id();
        let id_b = fresh_id();
        handle_should_offer(&id_a, "call1", "aaa", "bbb", false);
        let effects = handle_should_offer(&id_b, "call2", "aaa", "bbb", false);
        assert!(matches!(effects.as_slice(), [CallEffect::SendBusy { .. }]), "{:?}", effects);
    }

    #[test]
    fn handle_should_offer_is_a_no_op_when_redelivered_for_the_same_call() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        let effects = handle_should_offer(&id, "call1", "aaa", "bbb", false);
        assert!(effects.is_empty(), "redelivered 'call' while already ringing must be a no-op: {effects:?}");
    }

    #[test]
    fn handle_should_offer_is_a_no_op_for_a_second_call_id_while_already_active() {
        // A peer's own simultaneous "call" (different call_id) must not
        // race this side's own in-flight offer for the same pairing.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        let effects = handle_should_offer(&id, "call2", "aaa", "bbb", false);
        assert!(effects.is_empty(), "a peer's own simultaneous 'call' must not race this side's own in-flight offer: {effects:?}");
    }

    #[test]
    fn handle_offer_sends_busy_when_a_different_pairing_owns_the_slot() {
        let _guard = reset_state_for_test();
        let id_a = fresh_id();
        let id_b = fresh_id();
        handle_offer(&id_a, "call1", "sdp", false);
        let effects = handle_offer(&id_b, "call2", "sdp", false);
        assert!(matches!(effects.as_slice(), [CallEffect::SendBusy { .. }]), "{:?}", effects);
    }

    #[test]
    fn handle_offer_is_a_no_op_when_already_active_with_a_real_peer_connection() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_offer(&id, "call1", "sdp", false);
        let effects = handle_offer(&id, "call1", "sdp", false);
        assert!(effects.is_empty(), "{effects:?}");
    }

    #[test]
    fn handle_offer_redelivered_while_ringing_is_a_no_op() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let first = handle_offer(&id, "call1", "sdp", true);
        assert!(matches!(first.as_slice(), [CallEffect::AcquireMedia, CallEffect::StartRinging { .. }]));
        let redelivered = handle_offer(&id, "call1", "sdp", true);
        assert!(redelivered.is_empty(), "a redelivered offer while ringing must not restart the countdown: {redelivered:?}");
    }

    // --- invariant #3: an offer completing a call this side already placed skips ringing ---

    #[test]
    fn handle_offer_applies_immediately_when_it_completes_a_call_this_side_already_placed_tiebreak_direction_a() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        // This side lost the tie-break and sent a plain "call"; the peer's
        // offer then arrives for the same call_id already recorded via
        // request_call.
        let result = request_call(&id, "bbb", "aaa", true, 0); // loses tiebreak, sends 'call'
        let call_id = result.call_id.unwrap();
        let effects = handle_offer(&id, &call_id, "peer-sdp", false);
        assert!(
            matches!(effects.as_slice(), [CallEffect::ApplyRemoteOffer { .. }]),
            "an offer completing this side's own outgoing call must apply immediately, no ring: {effects:?}"
        );
    }

    #[test]
    fn handle_offer_applies_immediately_when_it_completes_a_call_this_side_already_placed_tiebreak_direction_b() {
        let _guard = reset_state_for_test();
        // Same as above, opposite tie-break direction — module-doc
        // invariant #3 only reproduces in one direction, so both need
        // covering.
        let id = fresh_id();
        let result = request_call(&id, "zzz", "aaa", true, 0); // loses tiebreak this time too, but with reversed pubkey ordering vs the test above
        let call_id = result.call_id.unwrap();
        let effects = handle_offer(&id, &call_id, "peer-sdp", false);
        assert!(matches!(effects.as_slice(), [CallEffect::ApplyRemoteOffer { .. }]), "{effects:?}");
    }

    #[test]
    fn handle_offer_fresh_call_starts_ringing_not_apply() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let effects = handle_offer(&id, "call1", "sdp", true);
        assert!(
            matches!(effects.as_slice(), [CallEffect::AcquireMedia, CallEffect::StartRinging { auto_answer: true, seconds_remaining: 5, .. }]),
            "{effects:?}"
        );
    }

    #[test]
    fn handle_offer_redelivered_after_accept_before_pc_exists_is_a_no_op() {
        // Without offer_applied, this exact sequence used to fall through
        // to a second ApplyRemoteOffer, indistinguishable from a tie-break
        // completion.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_offer(&id, "call1", "sdp", true);
        accept_incoming_call(0).expect("a pending offer should exist");
        let redelivered = handle_offer(&id, "call1", "sdp", true);
        assert!(redelivered.is_empty(), "an offer redelivered after accept but before the shell's PeerConnection exists must be a no-op: {redelivered:?}");
    }

    // --- accept / answer / ICE ---

    #[test]
    fn accept_incoming_call_hands_back_offer_and_buffered_ice_then_clears_pending() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_offer(&id, "call1", "the-sdp", false);
        let ice_outcome = handle_remote_ice(&id, "call1", Some("mid"), 0, "candidate1");
        assert_eq!(ice_outcome, IceOutcome::Buffered);
        let outcome = accept_incoming_call(0).expect("a pending offer should exist");
        assert_eq!(
            outcome,
            AcceptOutcome::ApplyOffer {
                pairing_id: id.clone(),
                call_id: "call1".to_string(),
                sdp: "the-sdp".to_string(),
                ice_buffer: vec![IceCandidate { sdp_mid: Some("mid".to_string()), sdp_m_line_index: 0, candidate: "candidate1".to_string() }],
            }
        );
        assert!(accept_incoming_call(0).is_none(), "accepting twice must be a no-op the second time");
    }

    #[test]
    fn handle_remote_ice_applies_when_matching_the_active_call_with_no_pending_offer() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        accept_incoming_call(0); // leaves the ring: active_pairing_id/call_id stay set, pending_offer clears
        let outcome = handle_remote_ice(&id, "call1", None, 1, "candidate2");
        assert!(matches!(outcome, IceOutcome::Apply { .. }), "{outcome:?}");
    }

    #[test]
    fn handle_remote_ice_drops_when_matching_neither_pending_nor_active() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let outcome = handle_remote_ice(&id, "no-such-call", None, 0, "candidate");
        assert_eq!(outcome, IceOutcome::Dropped);
    }

    #[test]
    fn should_apply_answer_true_once_then_false_on_redelivery() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        accept_incoming_call(0); // ringing -> claimed: a real answer can only ever arrive once an offer's actually been sent
        assert!(should_apply_answer(&id, "call1"));
        assert!(!should_apply_answer(&id, "call1"), "a redelivered answer must not be applied twice");
    }

    #[test]
    fn should_apply_answer_is_false_while_still_ringing() {
        // See should_apply_answer's own doc for why this can't be true
        // while still ringing.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        assert!(!should_apply_answer(&id, "call1"));
    }

    #[test]
    fn should_apply_answer_false_for_wrong_pairing_or_call_id() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        assert!(!should_apply_answer(&id, "wrong-call"));
        assert!(!should_apply_answer("wrong-pairing", "call1"));
    }

    // --- countdown ---

    #[test]
    fn tick_incoming_call_countdown_reaches_should_accept_after_five_ticks() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_offer(&id, "call1", "sdp", true);
        // Four Continue ticks, then ShouldAccept on the fifth.
        for expected in [4, 3, 2, 1] {
            match tick_incoming_call_countdown(&id, "call1") {
                TickOutcome::Continue { seconds_remaining } => assert_eq!(seconds_remaining, expected),
                other => panic!("expected Continue({expected}), got {other:?}"),
            }
        }
        assert_eq!(tick_incoming_call_countdown(&id, "call1"), TickOutcome::ShouldAccept);
    }

    #[test]
    fn tick_incoming_call_countdown_is_stale_after_accept() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_offer(&id, "call1", "sdp", true);
        accept_incoming_call(0);
        assert_eq!(tick_incoming_call_countdown(&id, "call1"), TickOutcome::Stale);
    }

    #[test]
    fn tick_incoming_call_countdown_is_stale_for_a_superseded_call() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_offer(&id, "call1", "sdp", true);
        assert_eq!(tick_incoming_call_countdown(&id, "call2"), TickOutcome::Stale);
    }

    // --- invariant #4: hang_up captures before clearing ---

    #[test]
    fn hang_up_sends_bye_using_the_captured_pairing_and_call_id() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        let effects = hang_up();
        assert!(
            matches!(
                effects.as_slice(),
                [CallEffect::ClearIncomingCallTimer, CallEffect::SendBye { pairing_id, call_id }, CallEffect::ClosePeerConnection]
                if pairing_id == &id && call_id == "call1"
            ),
            "{effects:?}"
        );
    }

    #[test]
    fn hang_up_with_nothing_active_sends_no_bye() {
        let _guard = reset_state_for_test();
        let effects = hang_up();
        assert_eq!(effects, vec![CallEffect::ClearIncomingCallTimer, CallEffect::ClosePeerConnection]);
    }

    #[test]
    fn hang_up_clears_state_so_a_second_hang_up_sends_no_bye() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        hang_up();
        let effects = hang_up();
        assert_eq!(effects, vec![CallEffect::ClearIncomingCallTimer, CallEffect::ClosePeerConnection]);
    }

    #[test]
    fn hang_up_cancels_a_deferred_call_so_a_later_peer_online_does_nothing() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        request_call(&id, "aaa", "bbb", false, 0); // deferred, peer offline
        hang_up();
        let effects = handle_peer_online(&id, "aaa", "bbb", 0);
        assert!(effects.is_empty(), "a cancelled deferred call must not fire once the peer comes online: {effects:?}");
    }

    // --- check_call_timeout ---

    #[test]
    fn check_call_timeout_is_a_no_op_before_the_deadline() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        request_call(&id, "aaa", "bbb", true, 1_000);
        assert!(check_call_timeout(1_000 + CALL_ANSWER_TIMEOUT_MS - 1).is_empty());
    }

    #[test]
    fn check_call_timeout_fires_at_the_deadline_with_hang_ups_effects_plus_show_call_outcome() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let call_id = request_call(&id, "aaa", "bbb", true, 1_000).call_id.unwrap();
        let effects = check_call_timeout(1_000 + CALL_ANSWER_TIMEOUT_MS);
        assert_eq!(
            effects,
            vec![
                CallEffect::ClearIncomingCallTimer,
                CallEffect::SendBye { pairing_id: id.clone(), call_id: call_id.clone() },
                CallEffect::ClosePeerConnection,
                CallEffect::ShowCallOutcome { pairing_id: id, call_id, reason: CallOutcomeReason::NoAnswer },
            ]
        );
        assert!(!any_call_wanted(), "the expired slot must actually clear, not just report effects");
    }

    #[test]
    fn check_call_timeout_never_fires_once_connected_once_is_set() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let call_id = request_call(&id, "aaa", "bbb", true, 1_000).call_id.unwrap();
        mark_connected(&id, &call_id);
        assert!(
            check_call_timeout(1_000 + CALL_ANSWER_TIMEOUT_MS + 1_000_000).is_empty(),
            "a call that has ever connected must never be timed out, no matter how long it's been claimed since"
        );
    }

    #[test]
    fn check_call_timeout_does_not_apply_to_ringing() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "bbb", "aaa", false); // this side wins the tiebreak -> Ringing, not Claimed
        assert!(
            check_call_timeout(i64::MAX).is_empty(),
            "an incoming ring is bounded by a human or auto-answer's own countdown, never by this"
        );
    }

    #[test]
    fn check_call_timeout_covers_a_deferred_call_resolved_via_handle_peer_online() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        request_call(&id, "aaa", "bbb", false, 1_000); // peer offline, deferred
        handle_peer_online(&id, "aaa", "bbb", 5_000); // resolves the deferred call, refreshes claimed_at_ms
        assert!(
            check_call_timeout(5_000 + CALL_ANSWER_TIMEOUT_MS - 1).is_empty(),
            "claimed_at_ms must be refreshed to the peer-online time, not stuck at the original deferred-since time"
        );
        let effects = check_call_timeout(5_000 + CALL_ANSWER_TIMEOUT_MS);
        assert!(!effects.is_empty());
    }

    // --- forget_pairing ---

    #[test]
    fn forget_pairing_clears_a_deferred_call_so_a_later_peer_online_does_nothing() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        request_call(&id, "aaa", "bbb", false, 0); // deferred, peer offline — still claims active_pairing_id
        assert!(any_call_wanted());
        let effects = forget_pairing(&id);
        assert_eq!(
            effects,
            vec![CallEffect::ClearIncomingCallTimer, CallEffect::ClosePeerConnection],
            "same unconditional ClosePeerConnection hang_up already emits for a deferred call — a no-op on the shell side since no real pc exists yet"
        );
        assert!(!any_call_wanted());
        let effects = handle_peer_online(&id, "aaa", "bbb", 0);
        assert!(effects.is_empty(), "a forgotten deferred call must not fire once the peer comes online: {effects:?}");
    }

    #[test]
    fn forget_pairing_closes_the_active_call_without_sending_bye() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        let effects = forget_pairing(&id);
        assert_eq!(
            effects,
            vec![CallEffect::ClearIncomingCallTimer, CallEffect::ClosePeerConnection],
            "no SendBye on delete, matching removePairing on both platforms"
        );
        assert!(handle_peer_hangup(&id, "call1").is_empty(), "active state should already be clear");
    }

    #[test]
    fn forget_pairing_is_a_no_op_for_an_unrelated_inactive_pairing() {
        let _guard = reset_state_for_test();
        let id_a = fresh_id();
        let id_b = fresh_id();
        handle_should_offer(&id_a, "call1", "aaa", "bbb", false);
        let effects = forget_pairing(&id_b);
        assert!(effects.is_empty());
        // id_a's own still-ringing call must be untouched.
        assert!(matches!(tick_incoming_call_countdown(&id_a, "call1"), TickOutcome::Continue { .. }));
    }

    // --- invariant #6: every teardown clears pending/timer state ---

    #[test]
    fn handle_peer_hangup_requires_both_pairing_and_call_id_to_match() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        assert!(handle_peer_hangup(&id, "wrong-call").is_empty(), "mismatched call_id must be a no-op");
        let effects = handle_peer_hangup(&id, "call1");
        assert_eq!(
            effects,
            vec![
                CallEffect::ClearIncomingCallTimer,
                CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::Cancelled },
                CallEffect::ClosePeerConnection,
            ]
        );
    }

    #[test]
    fn handle_peer_left_matches_on_pairing_id_alone() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        let effects = handle_peer_left(&id);
        assert_eq!(
            effects,
            vec![
                CallEffect::ClearIncomingCallTimer,
                CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::Cancelled },
                CallEffect::ClosePeerConnection,
            ]
        );
    }

    #[test]
    fn handle_peer_left_no_ops_for_an_inactive_pairing() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        assert!(handle_peer_left(&id).is_empty());
    }

    #[test]
    fn handle_peer_hangup_clears_a_pending_ring_too() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_offer(&id, "call1", "sdp", true);
        handle_peer_hangup(&id, "call1");
        assert_eq!(tick_incoming_call_countdown(&id, "call1"), TickOutcome::Stale, "pending_offer must be cleared");
        assert!(accept_incoming_call(0).is_none());
    }

    // --- CallEffect::ShowCallOutcome ---

    fn outcome_of(effects: &[CallEffect]) -> CallOutcomeReason {
        effects
            .iter()
            .find_map(|e| match e {
                CallEffect::ShowCallOutcome { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no outcome in {effects:?}"))
    }

    #[test]
    fn a_bye_means_cancelled_declined_or_ended_depending_on_what_it_ended() {
        let _guard = reset_state_for_test();
        // We were ringing: the caller gave up (a missed call).
        let ringing = fresh_id();
        handle_should_offer(&ringing, "call1", "aaa", "bbb", false);
        assert_eq!(outcome_of(&handle_peer_hangup(&ringing, "call1")), CallOutcomeReason::Cancelled);

        // We were calling and nothing had been exchanged: they said no.
        let calling = fresh_id();
        let call_id = request_call(&calling, "aaa", "bbb", true, 0).call_id.unwrap();
        assert_eq!(outcome_of(&handle_peer_hangup(&calling, &call_id)), CallOutcomeReason::Declined);

        // Anything past that — an answer applied, or connected — is an ended call.
        let answered = fresh_id();
        let call_id = request_call(&answered, "aaa", "bbb", true, 0).call_id.unwrap();
        assert!(should_apply_answer(&answered, &call_id));
        assert_eq!(outcome_of(&handle_peer_hangup(&answered, &call_id)), CallOutcomeReason::PeerEnded);

        let connected = fresh_id();
        handle_should_offer(&connected, "call3", "aaa", "bbb", false);
        accept_incoming_call(0);
        mark_connected(&connected, "call3");
        assert_eq!(outcome_of(&handle_peer_hangup(&connected, "call3")), CallOutcomeReason::PeerEnded);
    }

    #[test]
    fn a_timeout_says_unreachable_no_answer_or_never_connected() {
        let _guard = reset_state_for_test();
        // The peer never showed up online.
        let offline = fresh_id();
        request_call(&offline, "aaa", "bbb", false, 0);
        assert_eq!(outcome_of(&check_call_timeout(CALL_ANSWER_TIMEOUT_MS)), CallOutcomeReason::Unreachable);

        // The peer was online and nothing came back.
        let silent = fresh_id();
        request_call(&silent, "aaa", "bbb", true, 0);
        assert_eq!(outcome_of(&check_call_timeout(CALL_ANSWER_TIMEOUT_MS)), CallOutcomeReason::NoAnswer);

        // They answered, but the media never came up.
        let answered = fresh_id();
        let call_id = request_call(&answered, "aaa", "bbb", true, 0).call_id.unwrap();
        should_apply_answer(&answered, &call_id);
        assert_eq!(outcome_of(&check_call_timeout(CALL_ANSWER_TIMEOUT_MS)), CallOutcomeReason::NeverConnected);
    }

    #[test]
    fn a_camera_failure_is_reported_as_one_not_as_a_network_problem() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        request_call(&id, "aaa", "bbb", true, 0);
        note_media_failure();
        assert_eq!(outcome_of(&peer_connection_closed()), CallOutcomeReason::CameraFailed);

        // Also while ringing, and also after having connected.
        let ringing = fresh_id();
        handle_should_offer(&ringing, "call1", "aaa", "bbb", false);
        note_media_failure();
        assert_eq!(outcome_of(&peer_connection_closed()), CallOutcomeReason::CameraFailed);
        let connected = fresh_id();
        handle_should_offer(&connected, "call2", "aaa", "bbb", false);
        accept_incoming_call(0);
        mark_connected(&connected, "call2");
        note_media_failure();
        assert_eq!(outcome_of(&peer_connection_closed()), CallOutcomeReason::CameraFailed);
    }

    #[test]
    fn a_camera_failure_does_not_leak_into_the_next_call() {
        let _guard = reset_state_for_test();
        let first = fresh_id();
        request_call(&first, "aaa", "bbb", true, 0);
        note_media_failure();
        hang_up(); // the call ended some other way first
        let second = fresh_id();
        request_call(&second, "aaa", "bbb", true, 0);
        assert_eq!(outcome_of(&peer_connection_closed()), CallOutcomeReason::NeverConnected);
        // With no call at all it is a no-op.
        note_media_failure();
        assert!(peer_connection_closed().is_empty());
    }

    #[test]
    fn handle_peer_left_reports_dropped_after_mark_connected() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        accept_incoming_call(0); // ringing -> claimed: mark_connected only applies once negotiation is actually underway
        mark_connected(&id, "call1");
        let effects = handle_peer_left(&id);
        assert!(
            matches!(effects.as_slice(), [_, CallEffect::ShowCallOutcome { reason: CallOutcomeReason::Dropped, .. }, _]),
            "{effects:?}"
        );
    }

    /// Once a teardown function has already decided an outcome, the
    /// shell's later peer_connection_closed() call must be a pure no-op.
    #[test]
    fn peer_connection_closed_is_a_no_op_after_hang_up_or_handle_peer_hangup_already_decided() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        hang_up();
        assert_eq!(peer_connection_closed(), vec![], "hang_up already fully decided this teardown");

        let id2 = fresh_id();
        handle_should_offer(&id2, "call2", "aaa", "bbb", false);
        handle_peer_hangup(&id2, "call2");
        assert_eq!(peer_connection_closed(), vec![], "handle_peer_hangup already fully decided this teardown");
    }

    // --- handle_peer_busy ---

    #[test]
    fn handle_peer_busy_releases_the_callers_slot_when_pairing_and_call_id_match() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        // "bbb" loses the tiebreak, landing in the SendCall
        // (waiting-for-answer) branch.
        let result = request_call(&id, "bbb", "aaa", true, 0);
        assert_eq!(result.effects, vec![CallEffect::AcquireMedia, CallEffect::SendCall { pairing_id: id.clone(), call_id: result.call_id.clone().unwrap() }]);
        let effects = handle_peer_busy(&id, result.call_id.as_deref().unwrap());
        assert_eq!(
            effects,
            vec![
                CallEffect::ClearIncomingCallTimer,
                CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: result.call_id.clone().unwrap(), reason: CallOutcomeReason::Busy },
                CallEffect::ClosePeerConnection,
            ]
        );
        // The slot must actually be free again -- a fresh call attempt to
        // a different pairing must not be rejected as "already busy".
        let other_id = fresh_id();
        let other_result = request_call(&other_id, "aaa", "ccc", true, 0);
        assert!(other_result.call_id.is_some());
    }

    #[test]
    fn handle_peer_busy_requires_both_pairing_and_call_id_to_match() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = request_call(&id, "aaa", "bbb", true, 0);
        assert!(handle_peer_busy(&id, "wrong-call").is_empty(), "mismatched call_id must be a no-op");
        assert!(handle_peer_busy("unrelated-pairing", &result.call_id.unwrap()).is_empty(), "mismatched pairing_id must be a no-op");
    }

    // --- peer_connection_closed ---

    #[test]
    fn peer_connection_closed_clears_active_state_and_is_idempotent() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        let effects = peer_connection_closed();
        assert_eq!(
            effects,
            vec![
                CallEffect::SendBye { pairing_id: id.clone(), call_id: "call1".to_string() },
                CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::NeverConnected },
            ]
        );
        assert!(handle_peer_hangup(&id, "call1").is_empty(), "active state should already be clear");
        assert_eq!(peer_connection_closed(), vec![], "must be a no-op when nothing was active");
    }

    #[test]
    fn peer_connection_closed_reports_dropped_after_mark_connected() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        accept_incoming_call(0); // ringing -> claimed: mark_connected only applies once negotiation is actually underway
        mark_connected(&id, "call1");
        let effects = peer_connection_closed();
        assert_eq!(
            effects,
            vec![
                CallEffect::SendBye { pairing_id: id.clone(), call_id: "call1".to_string() },
                CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::Dropped },
            ]
        );
    }

    #[test]
    fn peer_connection_closed_clears_a_concurrent_pending_offer_too() {
        // A still-ringing offer also gets cleared and reported
        // NeverConnected — a ring, by definition, was never connected.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_offer(&id, "call1", "sdp", true);
        let effects = peer_connection_closed();
        assert_eq!(
            effects,
            vec![
                CallEffect::SendBye { pairing_id: id.clone(), call_id: "call1".to_string() },
                CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::NeverConnected },
            ],
            "{effects:?}"
        );
        assert_eq!(tick_incoming_call_countdown(&id, "call1"), TickOutcome::Stale, "pending_offer must be cleared too now");
    }

    #[test]
    fn peer_connection_closed_sends_a_bye_so_a_media_failure_doesnt_strand_the_peer() {
        // Found live: a caller whose own getUserMedia was denied reached
        // this function as its only teardown step (not a follow-up after
        // hang_up/handle_peer_hangup already ran) — the peer must be told,
        // or it's left ringing/calling forever.
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let call_id = request_call(&id, "aaa", "bbb", true, 0).call_id.unwrap();
        let effects = peer_connection_closed();
        assert!(
            matches!(effects.as_slice(), [CallEffect::SendBye { .. }, CallEffect::ShowCallOutcome { .. }]),
            "{effects:?}"
        );
        assert_eq!(effects[0], CallEffect::SendBye { pairing_id: id, call_id });
    }

    // --- mark_connected ---

    #[test]
    fn mark_connected_sets_connected_once_only_for_the_matching_call() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        // Mismatched pairing/call id — must not set connected_once.
        mark_connected("wrong-pairing", "call1");
        mark_connected(&id, "wrong-call");
        assert_eq!(
            peer_connection_closed(),
            vec![
                CallEffect::SendBye { pairing_id: id.clone(), call_id: "call1".to_string() },
                CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::NeverConnected },
            ]
        );
    }

    // --- media failure guard ---

    #[test]
    fn should_end_call_on_media_failure_matches_has_active_peer_connection() {
        let _guard = reset_state_for_test();
        assert!(should_end_call_on_media_failure(true));
        assert!(!should_end_call_on_media_failure(false));
    }

    // --- any_call_wanted ---

    #[test]
    fn any_call_wanted_reflects_deferred_calls() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        assert!(!any_call_wanted());
        request_call(&id, "aaa", "bbb", false, 0); // peer offline, defers
        assert!(any_call_wanted());
        hang_up(); // cancels the deferred call
        assert!(!any_call_wanted());
    }

    // --- is_call_active ---

    #[test]
    fn is_call_active_is_true_the_moment_the_slot_is_claimed_even_while_only_deferred() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        assert!(!is_call_active());
        // Peer offline defers the call, but invariant #1 still claims the
        // slot immediately (not once accepted/connected).
        request_call(&id, "aaa", "bbb", false, 0);
        assert!(is_call_active());
        assert!(any_call_wanted());
        hang_up();
        assert!(!is_call_active());

        // Peer online, slot claimed the same way, no deferred want at all.
        request_call(&id, "aaa", "bbb", true, 0);
        assert!(is_call_active());
        hang_up();
        assert!(!is_call_active());
    }

    // --- random_call_id ---

    #[test]
    fn random_call_id_is_eight_hex_chars() {
        let _guard = reset_state_for_test();
        let id = random_call_id();
        assert_eq!(id.len(), 8);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_deferred_call_asks_the_next_heartbeat_to_say_hello() {
        let _guard = reset_state_for_test();
        let _ = crate::presence::take_hello(); // consume the start-of-process hello
        let id = fresh_id();
        request_call(&id, "aaa", "bbb", false, 0);
        assert!(crate::presence::take_hello(), "a call to a peer we think is offline asks it to answer");
    }

    #[test]
    fn a_call_to_an_online_peer_does_not_ask_for_a_hello() {
        let _guard = reset_state_for_test();
        let _ = crate::presence::take_hello();
        let id = fresh_id();
        request_call(&id, "aaa", "bbb", true, 0);
        assert!(!crate::presence::take_hello());
    }

    // --- can_place_call ---

    #[test]
    fn can_place_call_needs_an_unconnected_contact_and_a_free_slot() {
        let _guard = reset_state_for_test();
        assert!(can_place_call(false));
        assert!(!can_place_call(true), "already connected to this contact");

        let id = fresh_id();
        request_call(&id, "aaa", "bbb", false, 0);
        assert!(!can_place_call(false), "the call slot is occupied");
        hang_up();
        assert!(can_place_call(false));
    }
}
