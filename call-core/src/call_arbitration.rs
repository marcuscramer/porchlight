//! Pure decision logic for the call-arbitration state machine, ported from
//! `CameraAgentService`/`NostrSignalingClient.kt` and `app.js`'s call
//! arbitration + WebRTC section — see `CALL_STATE.md` for the invariants
//! this is checked against.
//!
//! **Single global slot, not a per-id registry**: only one call is ever
//! active device-wide, regardless of how many pairings exist, so
//! [`CallState`] is one struct, not a `HashMap`.
//!
//! Lives behind `crate::STATE`'s shared lock, not one of its own.
//! [`request_call`]/[`handle_offer`]/etc. stay the locking entry points the
//! shell calls; a few functions [`presence`] needs to call while already
//! holding the lock have a `pub(crate) inner_*` twin that takes
//! `&mut CallState` directly instead of re-locking.
//!
//! **What this module doesn't own**: the real `PeerConnection` object;
//! `hasActivePeerConnection` (passed in, snapshotted by the shell, never
//! inferred here); wake locks/foreground-service bring-up (Android-only);
//! and presence/online tracking itself — this module only owns what happens
//! *once* a peer is known to be online, via [`handle_peer_online`].

use serde::Serialize;
#[cfg(test)]
use std::sync::Mutex;

/// Mirrors `CameraAgentService.AUTO_ANSWER_COUNTDOWN_SECONDS`/`app.js`'s
/// constant of the same name.
const AUTO_ANSWER_COUNTDOWN_SECONDS: u32 = 5;

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
}

impl CallState {
    pub(crate) fn new() -> Self {
        CallState { slot: CallSlot::Idle, deferred_call: None }
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
}

/// Why a call ended, for [`CallEffect::ShowCallOutcome`] — not every
/// teardown path emits this (`hang_up`/`forget_pairing` never do).
/// `PeerEnded` is the one case with an explicit signal (a real `"bye"`);
/// the other two are distinguished only by [`ActiveCall::connected_once`].
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcomeReason {
    PeerEnded,
    NeverConnected,
    Dropped,
}

/// Claims the call slot for `pairing_id`/`call_id` and tells the shell to
/// create an offer immediately, no ring — shared by the winning half of
/// [`request_call`]'s/[`handle_peer_online`]'s pubkey tie-break, where a
/// human already tapped Call so there's nothing left to ring for. **Not**
/// used by [`handle_should_offer`] — that path goes through
/// [`start_ringing`] instead; see [`PendingOfferKind`]'s doc for why.
fn claim_slot_and_create_offer(state: &mut CallState, pairing_id: &str, call_id: &str) -> Vec<CallEffect> {
    state.slot = CallSlot::Claimed(ActiveCall {
        pairing_id: pairing_id.to_string(),
        call_id: call_id.to_string(),
        answer_applied: false,
        offer_applied: false,
        connected_once: false,
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
pub fn request_call(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, peer_online: bool) -> RequestCallResult {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    if let Some(active) = state.slot.pairing_id() {
        if active != pairing_id {
            return RequestCallResult { call_id: None, effects: vec![] };
        }
    }
    let call_id = random_call_id();

    if !peer_online {
        state.deferred_call = Some(DeferredCall { pairing_id: pairing_id.to_string(), call_id: call_id.clone() });
        state.slot = CallSlot::Claimed(ActiveCall {
            pairing_id: pairing_id.to_string(),
            call_id: call_id.clone(),
            answer_applied: false,
            offer_applied: false,
            connected_once: false,
        });
        return RequestCallResult { call_id: Some(call_id), effects: vec![CallEffect::AcquireMedia] };
    }

    if wins_tiebreak(own_pubkey_hex, peer_pubkey_hex) {
        let mut effects = vec![CallEffect::AcquireMedia];
        effects.extend(claim_slot_and_create_offer(state, pairing_id, &call_id));
        return RequestCallResult { call_id: Some(call_id), effects };
    }

    state.slot = CallSlot::Claimed(ActiveCall {
        pairing_id: pairing_id.to_string(),
        call_id: call_id.clone(),
        answer_applied: false,
        offer_applied: false,
        connected_once: false,
    });
    RequestCallResult {
        call_id: Some(call_id.clone()),
        effects: vec![CallEffect::AcquireMedia, CallEffect::SendCall { pairing_id: pairing_id.to_string(), call_id }],
    }
}

/// Called by the shell's presence layer the moment a peer transitions
/// online — resolves any [`DeferredCall`] waiting on this pairing. `[]` if
/// none was actually wanted.
///
/// Locking wrapper around [`inner_handle_peer_online`] for `android.rs`/
/// `wasm.rs`; `presence` itself calls [`inner_handle_peer_online`] directly
/// since it's already holding `crate::STATE`'s guard.
pub fn handle_peer_online(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str) -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    inner_handle_peer_online(&mut app.call, pairing_id, own_pubkey_hex, peer_pubkey_hex)
}

pub(crate) fn inner_handle_peer_online(state: &mut CallState, pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str) -> Vec<CallEffect> {
    if !state.deferred_call.as_ref().is_some_and(|d| d.pairing_id == pairing_id) {
        return vec![];
    }
    let call_id = state.deferred_call.take().unwrap().call_id;

    if wins_tiebreak(own_pubkey_hex, peer_pubkey_hex) {
        claim_slot_and_create_offer(state, pairing_id, &call_id)
    } else {
        state.slot = CallSlot::Claimed(ActiveCall {
            pairing_id: pairing_id.to_string(),
            call_id: call_id.clone(),
            answer_applied: false,
            offer_applied: false,
            connected_once: false,
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
/// accept the ring (see [`PendingOfferKind`]'s doc for the bug this fixes).
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
/// pubkey-tie-break fast-path (`CALL_STATE.md` invariant #3: an offer that
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
/// Only ever `true` while [`CallSlot::Claimed`], never
/// [`CallSlot::Ringing`] — a real answer can only be a reply to an offer
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
/// things [`PendingOfferKind`] describes it needs to do now.
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
pub fn accept_incoming_call() -> Option<AcceptOutcome> {
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
/// [`Continue`]), the same way pairing-bootstrap's `handle_timeout` already
/// works (core decides, shell schedules). The shell calls this once per
/// second starting one second *after* [`handle_offer`] returned
/// [`StartRinging`] with `seconds_remaining: 5` — never a synchronous
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

/// **The `CALL_STATE.md` invariant #4 function.** Captures `pairing_id`/
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
/// bye) plus [`CallEffect::ShowCallOutcome`] with
/// [`CallOutcomeReason::PeerEnded`] — an explicit "bye" is the most
/// specific signal available, reported regardless of
/// [`ActiveCall::connected_once`].
pub fn handle_peer_hangup(pairing_id: &str, call_id: &str) -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    if !end_active_call_if_matching(state, pairing_id, call_id) {
        return vec![];
    }
    vec![
        CallEffect::ClearIncomingCallTimer,
        CallEffect::ShowCallOutcome { pairing_id: pairing_id.to_string(), call_id: call_id.to_string(), reason: CallOutcomeReason::PeerEnded },
        CallEffect::ClosePeerConnection,
    ]
}

/// Mirrors receiving a `"busy"` reply to our own outgoing call attempt —
/// releases the caller's own claimed slot (without this, the caller would
/// be stuck on the calling screen with no way back to idle). Same gating
/// as [`handle_peer_hangup`], but **deliberately no**
/// [`CallEffect::ShowCallOutcome`] — a busy reply already has its own
/// transient contact-badge UI; a full-screen "call ended" prompt on top
/// would be misleading, since the callee isn't reachable at all right now.
pub fn handle_peer_busy(pairing_id: &str, call_id: &str) -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    inner_handle_peer_busy(&mut app.call, pairing_id, call_id)
}

pub(crate) fn inner_handle_peer_busy(state: &mut CallState, pairing_id: &str, call_id: &str) -> Vec<CallEffect> {
    if !end_active_call_if_matching(state, pairing_id, call_id) {
        return vec![];
    }
    vec![CallEffect::ClearIncomingCallTimer, CallEffect::ClosePeerConnection]
}

/// Mirrors `onPeerLeft` — **no `call_id` check, unlike `bye`'s
/// call_id-scoped guard**: a peer going fully offline ends *any* call with
/// them regardless of which `call_id`. Confirmed intentional on both
/// platforms, not a bug to fix. Reports [`CallOutcomeReason::NeverConnected`]
/// or `::Dropped` based on [`ActiveCall::connected_once`] — the best signal
/// available for a peer that vanished from presence without an explicit
/// "bye".
pub fn handle_peer_left(pairing_id: &str) -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    inner_handle_peer_left(&mut app.call, pairing_id)
}

pub(crate) fn inner_handle_peer_left(state: &mut CallState, pairing_id: &str) -> Vec<CallEffect> {
    if state.slot.pairing_id() != Some(pairing_id) {
        return vec![];
    }
    let call_id = state.slot.call_id().unwrap_or_default().to_string();
    let was_connected = matches!(&state.slot, CallSlot::Claimed(active) if active.connected_once);
    state.slot = CallSlot::Idle;
    vec![
        CallEffect::ClearIncomingCallTimer,
        CallEffect::ShowCallOutcome {
            pairing_id: pairing_id.to_string(),
            call_id,
            reason: if was_connected { CallOutcomeReason::Dropped } else { CallOutcomeReason::NeverConnected },
        },
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
/// isn't already [`CallSlot::Idle`]. Every other teardown path already
/// clears the slot (and, where relevant, already emits its own outcome)
/// before its own `ClosePeerConnection` reaches the shell — so by the time
/// the shell's own teardown calls this function, it correctly no-ops unless
/// this really is a spontaneous drop with no prior call-core decision
/// behind it. That's what guarantees exactly one `ShowCallOutcome` per
/// call, never zero, never two.
pub fn peer_connection_closed() -> Vec<CallEffect> {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.call;
    match std::mem::replace(&mut state.slot, CallSlot::Idle) {
        CallSlot::Idle => vec![],
        CallSlot::Ringing(pending) => vec![CallEffect::ShowCallOutcome {
            pairing_id: pending.pairing_id,
            call_id: pending.call_id,
            reason: CallOutcomeReason::NeverConnected,
        }],
        CallSlot::Claimed(active) => vec![CallEffect::ShowCallOutcome {
            pairing_id: active.pairing_id,
            call_id: active.call_id,
            reason: if active.connected_once { CallOutcomeReason::Dropped } else { CallOutcomeReason::NeverConnected },
        }],
    }
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
        let result = request_call(&id, "aaa", "bbb", true); // "aaa" < "bbb"
        assert!(result.call_id.is_some());
        assert!(matches!(result.effects.as_slice(), [CallEffect::AcquireMedia, CallEffect::CreateOffer { .. }]), "{:?}", result.effects);
    }

    #[test]
    fn request_call_claims_slot_and_sends_call_when_peer_online_and_own_pubkey_loses_tiebreak() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = request_call(&id, "bbb", "aaa", true); // "bbb" > "aaa"
        assert!(result.call_id.is_some());
        assert!(matches!(result.effects.as_slice(), [CallEffect::AcquireMedia, CallEffect::SendCall { .. }]), "{:?}", result.effects);
    }

    #[test]
    fn request_call_defers_when_peer_offline_and_resolves_via_handle_peer_online_tiebreak_win() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = request_call(&id, "aaa", "bbb", false);
        assert!(result.call_id.is_some());
        assert_eq!(result.effects, vec![CallEffect::AcquireMedia]);
        let effects = handle_peer_online(&id, "aaa", "bbb");
        assert!(matches!(effects.as_slice(), [CallEffect::CreateOffer { .. }]), "{:?}", effects);
    }

    #[test]
    fn request_call_defers_when_peer_offline_and_resolves_via_handle_peer_online_tiebreak_loss() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        request_call(&id, "bbb", "aaa", false);
        let effects = handle_peer_online(&id, "bbb", "aaa");
        assert!(matches!(effects.as_slice(), [CallEffect::SendCall { .. }]), "{:?}", effects);
    }

    #[test]
    fn handle_peer_online_is_a_no_op_when_nothing_was_deferred() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        assert!(handle_peer_online(&id, "aaa", "bbb").is_empty());
    }

    #[test]
    fn request_call_no_ops_when_a_different_pairing_already_owns_the_slot() {
        let _guard = reset_state_for_test();
        let id_a = fresh_id();
        let id_b = fresh_id();
        request_call(&id_a, "aaa", "bbb", true);
        let result = request_call(&id_b, "aaa", "bbb", true);
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
        let outcome = accept_incoming_call().expect("a pending ring should exist");
        assert_eq!(outcome, AcceptOutcome::CreateOffer { pairing_id: id.clone(), call_id: "call1".to_string() });
        assert!(accept_incoming_call().is_none(), "accepting twice must be a no-op the second time");
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
        let result = request_call(&id, "bbb", "aaa", true); // loses tiebreak, sends 'call'
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
        // Same as above, opposite tie-break direction — CALL_STATE.md
        // invariant #3 only reproduces in one direction, so both need
        // covering.
        let id = fresh_id();
        let result = request_call(&id, "zzz", "aaa", true); // loses tiebreak this time too, but with reversed pubkey ordering vs the test above
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
        accept_incoming_call().expect("a pending offer should exist");
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
        let outcome = accept_incoming_call().expect("a pending offer should exist");
        assert_eq!(
            outcome,
            AcceptOutcome::ApplyOffer {
                pairing_id: id.clone(),
                call_id: "call1".to_string(),
                sdp: "the-sdp".to_string(),
                ice_buffer: vec![IceCandidate { sdp_mid: Some("mid".to_string()), sdp_m_line_index: 0, candidate: "candidate1".to_string() }],
            }
        );
        assert!(accept_incoming_call().is_none(), "accepting twice must be a no-op the second time");
    }

    #[test]
    fn handle_remote_ice_applies_when_matching_the_active_call_with_no_pending_offer() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        accept_incoming_call(); // leaves the ring: active_pairing_id/call_id stay set, pending_offer clears
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
        accept_incoming_call(); // ringing -> claimed: a real answer can only ever arrive once an offer's actually been sent
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
        accept_incoming_call();
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
        request_call(&id, "aaa", "bbb", false); // deferred, peer offline
        hang_up();
        let effects = handle_peer_online(&id, "aaa", "bbb");
        assert!(effects.is_empty(), "a cancelled deferred call must not fire once the peer comes online: {effects:?}");
    }

    // --- forget_pairing ---

    #[test]
    fn forget_pairing_clears_a_deferred_call_so_a_later_peer_online_does_nothing() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        request_call(&id, "aaa", "bbb", false); // deferred, peer offline — still claims active_pairing_id
        assert!(any_call_wanted());
        let effects = forget_pairing(&id);
        assert_eq!(
            effects,
            vec![CallEffect::ClearIncomingCallTimer, CallEffect::ClosePeerConnection],
            "same unconditional ClosePeerConnection hang_up already emits for a deferred call — a no-op on the shell side since no real pc exists yet"
        );
        assert!(!any_call_wanted());
        let effects = handle_peer_online(&id, "aaa", "bbb");
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
                CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::PeerEnded },
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
                CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::NeverConnected },
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
        assert!(accept_incoming_call().is_none());
    }

    // --- CallEffect::ShowCallOutcome ---

    #[test]
    fn handle_peer_hangup_reports_peer_ended_regardless_of_connected_once() {
        let _guard = reset_state_for_test();
        // Without a prior mark_connected — still PeerEnded, not NeverConnected.
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        let effects = handle_peer_hangup(&id, "call1");
        assert!(
            matches!(effects.as_slice(), [_, CallEffect::ShowCallOutcome { reason: CallOutcomeReason::PeerEnded, .. }, _]),
            "{effects:?}"
        );

        // With a prior mark_connected — still PeerEnded, not Dropped: "bye" always wins.
        let id2 = fresh_id();
        handle_should_offer(&id2, "call2", "aaa", "bbb", false);
        mark_connected(&id2, "call2");
        let effects = handle_peer_hangup(&id2, "call2");
        assert!(
            matches!(effects.as_slice(), [_, CallEffect::ShowCallOutcome { reason: CallOutcomeReason::PeerEnded, .. }, _]),
            "{effects:?}"
        );
    }

    #[test]
    fn handle_peer_left_reports_dropped_after_mark_connected() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        accept_incoming_call(); // ringing -> claimed: mark_connected only applies once negotiation is actually underway
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
        let result = request_call(&id, "bbb", "aaa", true);
        assert_eq!(result.effects, vec![CallEffect::AcquireMedia, CallEffect::SendCall { pairing_id: id.clone(), call_id: result.call_id.clone().unwrap() }]);
        let effects = handle_peer_busy(&id, &result.call_id.unwrap());
        assert_eq!(effects, vec![CallEffect::ClearIncomingCallTimer, CallEffect::ClosePeerConnection]);
        // The slot must actually be free again -- a fresh call attempt to
        // a different pairing must not be rejected as "already busy".
        let other_id = fresh_id();
        let other_result = request_call(&other_id, "aaa", "ccc", true);
        assert!(other_result.call_id.is_some());
    }

    #[test]
    fn handle_peer_busy_requires_both_pairing_and_call_id_to_match() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        let result = request_call(&id, "aaa", "bbb", true);
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
            vec![CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::NeverConnected }]
        );
        assert!(handle_peer_hangup(&id, "call1").is_empty(), "active state should already be clear");
        assert_eq!(peer_connection_closed(), vec![], "must be a no-op when nothing was active");
    }

    #[test]
    fn peer_connection_closed_reports_dropped_after_mark_connected() {
        let _guard = reset_state_for_test();
        let id = fresh_id();
        handle_should_offer(&id, "call1", "aaa", "bbb", false);
        accept_incoming_call(); // ringing -> claimed: mark_connected only applies once negotiation is actually underway
        mark_connected(&id, "call1");
        let effects = peer_connection_closed();
        assert_eq!(
            effects,
            vec![CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::Dropped }]
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
            vec![CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::NeverConnected }],
            "{effects:?}"
        );
        assert_eq!(tick_incoming_call_countdown(&id, "call1"), TickOutcome::Stale, "pending_offer must be cleared too now");
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
        assert_eq!(peer_connection_closed(), vec![CallEffect::ShowCallOutcome { pairing_id: id.clone(), call_id: "call1".to_string(), reason: CallOutcomeReason::NeverConnected }]);
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
        request_call(&id, "aaa", "bbb", false); // peer offline, defers
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
        request_call(&id, "aaa", "bbb", false);
        assert!(is_call_active());
        assert!(any_call_wanted());
        hang_up();
        assert!(!is_call_active());

        // Peer online, slot claimed the same way, no deferred want at all.
        request_call(&id, "aaa", "bbb", true);
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
}
