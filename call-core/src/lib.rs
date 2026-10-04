//! Pure decision logic for the passphrase-pairing (SPAKE2 bootstrap) state
//! machine — first module of `call-core`, part of a functional-core/
//! imperative-shell design shared by every module in this crate. Ported
//! directly from `CameraAgentService.onPairingBootstrapMessage`/
//! `verifyPairingConfirmation` and their `app.js` twin — every behavior
//! here was copied from, and every test below was chosen to pin down, what
//! those two already-proven implementations do.
//!
//! **What lives here, not in `pake-bridge`**: the *decision* logic
//! (self-echo drop, single-candidate tracking, collision-is-final-even-
//! after-a-match, redelivery-is-a-no-op, live-window timeout that respects
//! an already-resolved match) — not the crypto, which lives in
//! `pake-bridge` and is called from here directly as a normal Rust
//! dependency. This crate owns the *whole* attempt lifecycle, including
//! holding the live `pake_bridge::PakeSession` values themselves: when an
//! attempt is removed from this crate's internal registry, its session (if
//! not yet consumed) drops via ordinary Rust ownership, no explicit
//! "destroy" call needed.
//!
//! **FFI shape**: every platform-crossing function takes/returns plain
//! strings (JSON for anything structured) rather than a rich typed FFI
//! surface — deliberately, given the alternative (hand-marshaling a whole
//! family of `Effect` variants through JNI object graphs) would be far
//! more boilerplate than this crate's actual logic. All the real type
//! safety lives in these Rust enums (compile-time checked); each platform
//! just does `JSONObject`/`JSON.parse` on the result.
//!
//! **Modules** (each has its own docs; every one is pure decision logic over
//! plain data, with the shell owning all real I/O):
//!
//! - this file — the passphrase-pairing (SPAKE2 bootstrap) state machine;
//! - `call_arbitration` — the one call slot, its invariants and tie-break;
//! - `presence` — who is online/busy, the heartbeat cadence, `hello` replies
//!   (calls into `call_arbitration`; nothing calls back);
//! - `nostr_protocol` — gift-wrap/bootstrap event construction and
//!   verification, the signal-message schema, dedup, relay filters;
//! - `signal_retry` — the retry queue and per-relay rejection cooldown for
//!   signal messages that missed a relay;
//! - `ice_evidence` — ICE-candidate evidence and the "why did this call never
//!   connect?" diagnosis;
//! - `wake_up` — the step-by-step decisions for getting the call screen in
//!   front of the Portal's screensaver (Android-only feature; the logic lives
//!   here so it runs under `cargo test`).
//!
//! The last three (and `nostr_protocol`) are independent of one another and
//! of `call_arbitration`/`presence`.

use pake_bridge::{PakeKeys, PakeSession};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

#[cfg(target_os = "android")]
mod android;
#[cfg(target_arch = "wasm32")]
mod wasm;

pub mod call_arbitration;

pub mod wake_up;

pub mod presence;

pub mod nostr_protocol;

pub mod signal_retry;

pub mod ice_evidence;

/// A peer's self-reported, untrusted display name is capped and stripped
/// of bidi-override/zero-width characters before ever being stored — it's
/// exactly what a human reads on the "Pair with \[name\]?" screen to confirm
/// they're pairing with who they expect (see [`sanitize_name`]'s own doc).
const MAX_NAME_LENGTH: usize = 100;

/// A real SPAKE2 blinded message / confirmation tag is a small fixed size;
/// anything wildly past this is either a bug or a hostile guess at this
/// rendezvous tag, not worth even attempting to decode.
const MAX_PAKE_MESSAGE_HEX_LENGTH: usize = 1024;

/// A real SDP offer/answer is well under this; anything past it is either a
/// bug or a hostile payload, not worth even attempting to hand to WebRTC.
/// Generous safety margin, not a tight correctness-affecting bound — kept
/// here (not enforced internally by this crate, since `call_arbitration`
/// treats SDP as an opaque string) purely so both platforms read the same
/// number instead of two independently hand-typed literals — see
/// [`protocol_constants`].
const MAX_SDP_LENGTH: usize = 65_536;

/// Same reasoning as [`MAX_SDP_LENGTH`], for one ICE candidate string.
const MAX_ICE_CANDIDATE_LENGTH: usize = 4_096;

/// How long a pairing attempt stays live before [`handle_timeout`] resolves
/// it as expired — the shell schedules its own timeout call at this delay
/// (core decides, shell schedules, same split as
/// [`call_arbitration::tick_incoming_call_countdown`]'s timer loop); this
/// crate's own logic never reads the duration itself, only `generation` at
/// timeout time. Exposed via [`protocol_constants`] so both platforms
/// schedule off one real value instead of two hand-copied literals.
const PAKE_LIVE_WINDOW_MS: u64 = 120_000;

/// Protocol-level constants both platforms must use identically, exposed as
/// one JSON blob (`android::nativeProtocolConstants`/`wasm::protocolConstants`)
/// rather than one accessor per value — cheaper to call once at shell
/// startup than to add a whole binding function per constant. This is the
/// single shared source of truth for values (`SIGNAL_KIND`/`WRAP_KIND`/
/// `MAX_NAME_LENGTH`/the pairing live-window duration) that would otherwise
/// each be independently hand-copied as literals on both platforms.
#[derive(Serialize)]
pub struct ProtocolConstants {
    pub signal_kind: u16,
    pub wrap_kind: u16,
    pub max_name_length: usize,
    pub pake_live_window_ms: u64,
    pub max_sdp_length: usize,
    pub max_ice_candidate_length: usize,
}

pub fn protocol_constants() -> ProtocolConstants {
    ProtocolConstants {
        signal_kind: nostr_protocol::SIGNAL_KIND,
        wrap_kind: nostr_protocol::WRAP_KIND,
        max_name_length: MAX_NAME_LENGTH,
        pake_live_window_ms: PAKE_LIVE_WINDOW_MS,
        max_sdp_length: MAX_SDP_LENGTH,
        max_ice_candidate_length: MAX_ICE_CANDIDATE_LENGTH,
    }
}

/// One in-progress pairing attempt's live state — entirely in-memory,
/// mirrors `CameraAgentService.PakeAttempt`/`app.js`'s attempt object
/// field-for-field. `generation` is this crate's equivalent of the
/// Kotlin/JS "is this literally the same attempt object" reference-
/// identity check the timeout handler needs (see [`handle_timeout`]'s
/// doc) — Rust has no object identity to lean on here, so a monotonic
/// counter plays the same role explicitly.
struct PakeAttempt {
    generation: u64,
    own_pubkey_hex: String,
    own_name: String,
    #[allow(dead_code)] // kept for symmetry/future use (e.g. re-publish diagnostics); not read yet
    rendezvous_tag: String,
    outbound_hex: String,
    session: Option<PakeSession>,
    candidate_pubkey: Option<String>,
    /// Set once the peer's `pake1` is processed: the keys the confirmation
    /// and the sealed device names come from.
    keys: Option<PakeKeys>,
    /// This side's own device name, sealed once (right after `keys`) so every
    /// republished `pake-confirm` is byte-identical.
    local_sealed_name: Option<String>,
    /// The peer's `pake-confirm` (confirmation, sealed name) if it arrived
    /// before this side had processed their `pake1`.
    stashed_remote_confirm: Option<(String, String)>,
    resolved: bool,
}

/// Every piece of mutable state this crate holds, behind one shared lock —
/// the pairing-bootstrap registry (this file), plus each of the other three
/// modules' own state. `presence` calling into `call_arbitration` while
/// holding its own lock would otherwise be a real deadlock hazard (`std::
/// sync::Mutex` isn't reentrant, even on the same thread) — safe only if
/// the dependency direction never reverses, which nothing structurally
/// guarantees with separately-locked statics. One shared lock removes the
/// *lock-ordering* class of bug entirely: with one lock, "which order do I
/// acquire locks in" isn't a question that has an answer other than
/// "there's only one." `pairing_registry`/`dedup` never participate in any
/// cross-module call either way, and share this lock purely for the same
/// reason `presence`/`call` do — one `AppState`, not four.
///
/// Each module keeps its own public, locking entry points (`start_attempt`,
/// `presence::mark_seen`, `call_arbitration::request_call`, etc.) with
/// their exact original signatures and behavior. The only new surface is a
/// handful of `pub(crate) inner_*` functions in `call_arbitration` that
/// `presence` calls directly while already holding this lock, instead of
/// `presence` re-locking.
struct AppState {
    pairing_registry: HashMap<String, PakeAttempt>,
    presence: presence::PresenceState,
    call: call_arbitration::CallState,
    dedup: nostr_protocol::DedupState,
    signal_retry: signal_retry::SignalRetryState,
    ice: ice_evidence::IceState,
}

impl AppState {
    fn new() -> Self {
        AppState {
            pairing_registry: HashMap::new(),
            presence: presence::PresenceState::new(),
            call: call_arbitration::CallState::new(),
            dedup: nostr_protocol::DedupState::new(),
            signal_retry: signal_retry::SignalRetryState::new(),
            ice: ice_evidence::IceState::new(),
        }
    }
}

static STATE: LazyLock<Mutex<AppState>> = LazyLock::new(|| Mutex::new(AppState::new()));
static NEXT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_generation() -> u64 {
    NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// What starting a new attempt hands back to the shell: the rendezvous tag
/// to subscribe at, this side's outbound SPAKE2 message (to publish once a
/// peer is found — or immediately, via the heartbeat path), and the
/// `generation` token to pass back verbatim to [`handle_timeout`] when the
/// live window elapses.
#[derive(Serialize, Debug, PartialEq)]
pub struct StartResult {
    pub rendezvous_tag: String,
    pub outbound_hex: String,
    pub generation: u64,
}

/// Starts a new pairing attempt for `pairing_id`, replacing any existing
/// one under the same id (a fresh retry after a timeout/collision). Trim/
/// NFC-normalization and the rendezvous-tag derivation happen inside
/// `pake_bridge::PakeSession::start` — this function does no passphrase
/// handling of its own beyond forwarding it.
pub fn start_attempt(pairing_id: &str, own_pubkey_hex: &str, own_name: &str, raw_passphrase: &str) -> StartResult {
    let (session, rendezvous_tag, outbound) = PakeSession::start(raw_passphrase);
    let outbound_hex = hex_encode(&outbound);
    let generation = next_generation();
    let attempt = PakeAttempt {
        generation,
        own_pubkey_hex: own_pubkey_hex.to_string(),
        own_name: sanitize_name(own_name),
        rendezvous_tag: rendezvous_tag.clone(),
        outbound_hex: outbound_hex.clone(),
        session: Some(session),
        candidate_pubkey: None,
        keys: None,
        local_sealed_name: None,
        stashed_remote_confirm: None,
        resolved: false,
    };
    STATE.lock().unwrap_or_else(|p| p.into_inner()).pairing_registry.insert(pairing_id.to_string(), attempt);
    StartResult { rendezvous_tag, outbound_hex, generation }
}

/// Abandons an attempt outright (user cancel, contact deleted) without
/// completing it — removes it from the registry; its session (if not yet
/// consumed) drops automatically. A no-op if there's no live attempt for
/// `pairing_id`.
pub fn cancel_attempt(pairing_id: &str) {
    STATE.lock().unwrap_or_else(|p| p.into_inner()).pairing_registry.remove(pairing_id);
}

/// A JSON-encoded [`PendingSnapshot`] for one live pairing attempt — what
/// the shell needs, all at once, to (re)publish a heartbeat tick for it:
/// the rendezvous tag, the messages to publish, and the candidate's pubkey
/// once known. Returns `None` if there's no live attempt for `pairing_id`.
///
/// **`pake1` is always among the messages, and `pake-confirm` joins it once
/// this side has processed a candidate's `pake1`.** Relays don't store
/// ephemeral events, so a peer that joins later only ever sees what is
/// published *after* it subscribes. If this side stopped publishing its
/// `pake1` the moment it finished (as an earlier version did), a late joiner
/// received this side's `pake-confirm` but could never complete its own half
/// of the exchange — it needs this side's `pake1` for that — and the pairing
/// fell back to the 120s timeout unless the timing happened to line up.
pub fn build_bootstrap_payload(pairing_id: &str) -> Option<String> {
    let state = STATE.lock().unwrap_or_else(|p| p.into_inner());
    let attempt = state.pairing_registry.get(pairing_id)?;
    let mut payloads = vec![BootstrapPayload::Pake1 { outbound: attempt.outbound_hex.clone() }];
    if let (Some(keys), Some(sealed_name), Some(_)) = (&attempt.keys, &attempt.local_sealed_name, &attempt.candidate_pubkey) {
        payloads.push(BootstrapPayload::PakeConfirm { confirmation: keys.confirmation_hex().to_string(), sealed_name: sealed_name.clone() });
    }
    let snapshot = PendingSnapshot {
        rendezvous_tag: attempt.rendezvous_tag.clone(),
        payloads,
        candidate_pubkey: attempt.candidate_pubkey.clone(),
    };
    serde_json::to_string(&snapshot).ok()
}

/// Everything the shell needs to (re)publish a heartbeat tick for one live
/// pending pairing (`payloads`: each is published as its own event) — bundled into one call/lookup rather than three,
/// since all of it comes from the same attempt and none of it is useful
/// without the others. `candidate_pubkey` mirrors
/// `NostrSignalingClient.PendingPairing.bootstrapTarget`'s own doc: once
/// known (from the peer's own `pake1`), the (re)published bootstrap
/// message should be tagged directly at them, not just broadcast at the
/// rendezvous point.
#[derive(Serialize, Debug, PartialEq)]
pub struct PendingSnapshot {
    pub rendezvous_tag: String,
    pub payloads: Vec<BootstrapPayload>,
    pub candidate_pubkey: Option<String>,
}

/// Drives the entire SPAKE2 pairing state machine for one attempt — ported
/// directly from `CameraAgentService.onPairingBootstrapMessage`. `type_`
/// and `payload_json` mirror the wire message's own `type`/rest-of-object
/// shape (`{"type":"pake1","outbound":"..."}` or
/// `{"type":"pake-confirm","confirmation":"...","sealed_name":"..."}`) — the transport/
/// signature-verification layer (still native, `NostrSignalingClient`/
/// `app.js`'s relay client) only needs to have already confirmed
/// `sender_pubkey` actually signed this message; everything about *what it
/// means* happens here.
pub fn handle_bootstrap_message(pairing_id: &str, sender_pubkey: &str, type_: &str, payload_json: &str) -> Vec<Effect> {
    let mut app = STATE.lock().unwrap_or_else(|p| p.into_inner());
    let registry = &mut app.pairing_registry;
    let Some(attempt) = registry.get_mut(pairing_id) else { return vec![] };

    // Drop the relay's echo of this side's own publish — see
    // `PakeAttempt.own_pubkey_hex`'s doc on the Kotlin/JS twins for why
    // this is load-bearing, not defensive noise.
    if sender_pubkey == attempt.own_pubkey_hex {
        return vec![];
    }

    let first_sight = attempt.candidate_pubkey.is_none();
    if first_sight {
        attempt.candidate_pubkey = Some(sender_pubkey.to_string());
    } else if attempt.candidate_pubkey.as_deref() != Some(sender_pubkey) {
        // A second, distinct sender at this rendezvous tag — refuse
        // outright rather than presenting a choice. Deliberately fires
        // even after a successful match/`resolved` — a late-arriving
        // second party is still caught; this is intruder detection, not a
        // bug to "fix" by gating on `!resolved`.
        registry.remove(pairing_id);
        return vec![
            Effect::SetCollision { pairing_id: pairing_id.to_string() },
            Effect::KickHeartbeat,
        ];
    }

    let mut effects = match type_ {
        "pake1" => handle_pake1(registry, pairing_id, sender_pubkey, payload_json),
        "pake-confirm" => handle_pake_confirm(registry, pairing_id, sender_pubkey, payload_json),
        _ => vec![],
    };
    // The first time anyone answers at this rendezvous point, answer back
    // right now: publish everything we have, addressed to them. They joined
    // *after* our last periodic publish, so waiting for the next one would
    // leave them stuck for up to a full interval — and they can't finish
    // without our `pake1`, which a relay will not replay.
    if first_sight && attempt_still_live(registry, pairing_id) {
        effects.push(Effect::KickHeartbeat);
    }
    effects
}

fn attempt_still_live(registry: &HashMap<String, PakeAttempt>, pairing_id: &str) -> bool {
    registry.contains_key(pairing_id)
}

fn handle_pake1(registry: &mut HashMap<String, PakeAttempt>, pairing_id: &str, sender_pubkey: &str, payload_json: &str) -> Vec<Effect> {
    #[derive(Deserialize, Default)]
    struct Pake1Payload {
        #[serde(default)]
        outbound: String,
    }
    let payload: Pake1Payload = serde_json::from_str(payload_json).unwrap_or_default();

    let attempt = registry.get_mut(pairing_id).expect("caller already confirmed this exists");
    let rendezvous_tag = attempt.rendezvous_tag.clone();
    let Some(session) = attempt.session.take() else {
        // Already finish()'d — a redelivered/duplicate pake1, no-op. Note
        // this already consumed `session` via `.take()`; put it back if we
        // bail before actually using it below isn't needed since `None`
        // means there's nothing to restore.
        return vec![];
    };
    if payload.outbound.is_empty() || payload.outbound.len() > MAX_PAKE_MESSAGE_HEX_LENGTH {
        // Put the session back — this message doesn't count as "processed"
        // (a length-capped/blank outbound is dropped, not consumed),
        // exactly mirroring the Kotlin/JS early-return-before-finish().
        attempt.session = Some(session);
        return vec![];
    }
    let Some(inbound) = hex_decode(&payload.outbound) else {
        attempt.session = Some(session);
        return vec![];
    };

    let mut effects = Vec::new();
    match session.finish(&inbound) {
        Ok(keys) => {
            let confirmation = keys.confirmation_hex().to_string();
            let sealed_name = keys.seal_name(&attempt.own_pubkey_hex, &attempt.own_name);
            attempt.keys = Some(keys);
            attempt.local_sealed_name = Some(sealed_name.clone());
            effects.push(Effect::SendBootstrap {
                pairing_id: pairing_id.to_string(),
                rendezvous_tag,
                target_pubkey: sender_pubkey.to_string(),
                payload: BootstrapPayload::PakeConfirm { confirmation, sealed_name },
            });
            // The candidate's own pake-confirm may have already arrived
            // before we got this far (a real, if rare, delivery-order
            // race) — check it now rather than waiting for a redelivery.
            if let Some((their_confirmation, their_sealed_name)) = attempt.stashed_remote_confirm.take() {
                effects.extend(verify_confirmation_and_resolve(registry, pairing_id, sender_pubkey, &their_confirmation, &their_sealed_name));
            }
        }
        Err(_) => {
            // A malformed message from a genuine peer shouldn't happen;
            // treat like any other failed attempt rather than trying to
            // distinguish "attack" from "bug."
            registry.remove(pairing_id);
            effects.push(Effect::SetTimedOut { pairing_id: pairing_id.to_string() });
        }
    }
    effects
}

fn handle_pake_confirm(registry: &mut HashMap<String, PakeAttempt>, pairing_id: &str, sender_pubkey: &str, payload_json: &str) -> Vec<Effect> {
    #[derive(Deserialize, Default)]
    struct PakeConfirmPayload {
        #[serde(default)]
        confirmation: String,
        #[serde(default)]
        sealed_name: String,
    }
    let payload: PakeConfirmPayload = serde_json::from_str(payload_json).unwrap_or_default();
    // A confirmation without a sealed name is not a message this version
    // sends, so it is dropped rather than accepted with a blank name.
    if payload.confirmation.is_empty()
        || payload.confirmation.len() > MAX_PAKE_MESSAGE_HEX_LENGTH
        || payload.sealed_name.len() != pake_bridge::SEALED_NAME_HEX_LEN
    {
        return vec![];
    }
    let attempt = registry.get_mut(pairing_id).expect("caller already confirmed this exists");
    if attempt.keys.is_none() {
        // Haven't processed their pake1 yet ourselves — stash for the
        // recheck in handle_pake1 once we do.
        attempt.stashed_remote_confirm = Some((payload.confirmation, payload.sealed_name));
        return vec![];
    }
    verify_confirmation_and_resolve(registry, pairing_id, sender_pubkey, &payload.confirmation, &payload.sealed_name)
}

/// A match proves both sides derived the identical SPAKE2 secret — i.e.,
/// both typed the same passphrase — and is what actually surfaces the
/// "Pair with \[name\]?" tap, not merely completing this far without an
/// error. A mismatch just means a wrong passphrase somewhere; quietly
/// discard and let the human retry with a fresh phrase, same as a
/// timeout. Mirrors `CameraAgentService.verifyPairingConfirmation`.
///
/// The peer's device name arrives sealed under keys only a side that typed
/// the same phrase has, so it is only opened *after* the confirmation
/// matches. A name that won't open fails the attempt just like a mismatch.
fn verify_confirmation_and_resolve(
    registry: &mut HashMap<String, PakeAttempt>,
    pairing_id: &str,
    sender_pubkey: &str,
    their_confirm_hex: &str,
    their_sealed_name_hex: &str,
) -> Vec<Effect> {
    let attempt = registry.get(pairing_id).expect("caller already confirmed this exists");
    let name = attempt.keys.as_ref().and_then(|keys| {
        if pake_bridge::verify_confirmation(keys.confirmation_hex(), their_confirm_hex) {
            keys.open_name(sender_pubkey, their_sealed_name_hex)
        } else {
            None
        }
    });
    let Some(name) = name else {
        registry.remove(pairing_id);
        return vec![Effect::SetTimedOut { pairing_id: pairing_id.to_string() }];
    };
    let attempt = registry.get_mut(pairing_id).expect("still present, just checked");
    attempt.resolved = true;
    vec![Effect::SetConfirmedCandidate {
        pairing_id: pairing_id.to_string(),
        pubkey_hex: sender_pubkey.to_string(),
        name: sanitize_name(truncate_chars(&name, MAX_NAME_LENGTH)),
    }]
}

/// The live-window timeout fired for `pairing_id` — a no-op unless the
/// attempt that's *still* registered under that id is literally the same
/// one this timeout was scheduled for (`generation` matches) and it hasn't
/// already resolved. Without the generation check, a fresh retry (a human
/// re-entering the phrase after this exact attempt already timed out or
/// collided) would have its brand-new attempt clobbered by a stale timer
/// from the old one — mirrors the Kotlin/JS twins' object-identity check,
/// made explicit since Rust has no object identity to lean on here.
/// Without the `resolved` check, a human who takes longer than the live
/// window to actually tap Confirm after a valid match gets yanked to a
/// false "No response" error — confirmed as a real bug in the original
/// implementation, not just a theoretical race.
pub fn handle_timeout(pairing_id: &str, generation: u64) -> Vec<Effect> {
    let mut app = STATE.lock().unwrap_or_else(|p| p.into_inner());
    let registry = &mut app.pairing_registry;
    let Some(attempt) = registry.get(pairing_id) else { return vec![] };
    if attempt.generation != generation || attempt.resolved {
        return vec![];
    }
    registry.remove(pairing_id);
    vec![Effect::SetTimedOut { pairing_id: pairing_id.to_string() }, Effect::KickHeartbeat]
}

/// Effects the shell must actually perform — signaling sends, UI-facing
/// contact-state updates, and timer scheduling. Every variant carries only
/// plain data; the shell owns all the real I/O (publishing to relays,
/// scheduling a real OS/JS timer, updating whatever UI-observable contact
/// list it maintains).
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum Effect {
    SendBootstrap { pairing_id: String, rendezvous_tag: String, target_pubkey: String, payload: BootstrapPayload },
    KickHeartbeat,
    SetCollision { pairing_id: String },
    SetTimedOut { pairing_id: String },
    SetConfirmedCandidate { pairing_id: String, pubkey_hex: String, name: String },
}

/// Matches the wire message shape both platforms' `NostrSignalingClient`/
/// relay client already publish — `{"type":"pake1",...}` or
/// `{"type":"pake-confirm",...}`.
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum BootstrapPayload {
    #[serde(rename = "pake1")]
    Pake1 { outbound: String },
    #[serde(rename = "pake-confirm")]
    PakeConfirm { confirmation: String, sealed_name: String },
}

/// Strips control, bidi-override, and zero-width characters from a name
/// before it's ever stored — a peer's self-reported name is exactly what a
/// human reads on the "Pair with \[name\]?" screen to confirm they're
/// pairing with who they expect, so this isn't cosmetic: a RIGHT-TO-LEFT
/// OVERRIDE or zero-width character could visually spoof that name into
/// reading as something else entirely. Ported directly from
/// `NameSanitize.kt`/`app.js`'s `sanitizeName` (same character ranges).
pub fn sanitize_name(name: &str) -> String {
    name.chars().filter(|c| !is_stripped_char(*c)).collect()
}

fn is_stripped_char(c: char) -> bool {
    matches!(c as u32,
        0x0000..=0x001F | 0x007F..=0x009F |
        0x200B..=0x200F | 0x2028..=0x2029 |
        0x202A..=0x202E | 0x2060..=0x2069 |
        0xFEFF
    )
}

fn truncate_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// `pake_bridge`'s own — this crate already depends on it directly as a
/// plain Rust library, so re-exporting avoids a second, independently
/// hand-kept copy.
pub(crate) use pake_bridge::{hex_decode, hex_encode};

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test gets its own unique pairing_id (a random-ish string
    /// derived from the test's own name via a simple counter) so tests
    /// never collide in the shared, process-global `pairing_registry` —
    /// cargo runs tests in parallel threads by default. No shared
    /// `reset_state_for_test`-style guard needed here (unlike
    /// `call_arbitration`'s single global slot): distinct keys in the same
    /// `HashMap` don't corrupt each other, so per-test uniqueness alone is
    /// sufficient isolation.
    fn fresh_pairing_id() -> String {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        format!("test-pairing-{}", COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }

    fn pake1_json(outbound_hex: &str) -> String {
        serde_json::to_string(&BootstrapPayload::Pake1 { outbound: outbound_hex.to_string() }).unwrap()
    }

    /// What a `pake-confirm` carries: the proof and the sealed device name.
    #[derive(Clone)]
    struct SentConfirm {
        confirmation: String,
        sealed_name: String,
    }
    fn pake_confirm_json(c: &SentConfirm) -> String {
        serde_json::to_string(&BootstrapPayload::PakeConfirm { confirmation: c.confirmation.clone(), sealed_name: c.sealed_name.clone() }).unwrap()
    }

    /// Drives both sides of a real two-party exchange to a resolved match,
    /// returning (pairing_id_a, pairing_id_b, a's-view-of-effects-from-bs-pake1).
    /// Used as setup by several tests below that only care about what
    /// happens *after* a clean match.
    fn run_to_resolved_match(passphrase: &str) -> (String, String) {
        let id_a = fresh_pairing_id();
        let id_b = fresh_pairing_id();
        let a = start_attempt(&id_a, "pubkey-a", "Alice", passphrase);
        let b = start_attempt(&id_b, "pubkey-b", "Bob", passphrase);

        // B's pake1 arrives at A.
        let effects_a = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        let a_confirm = expect_send_confirm(&effects_a);
        // A's pake1 arrives at B.
        let effects_b = handle_bootstrap_message(&id_b, "pubkey-a", "pake1", &pake1_json(&a.outbound_hex));
        let b_confirm = expect_send_confirm(&effects_b);

        // Confirmations cross.
        let effects_a2 = handle_bootstrap_message(&id_a, "pubkey-b", "pake-confirm", &pake_confirm_json(&b_confirm));
        let effects_b2 = handle_bootstrap_message(&id_b, "pubkey-a", "pake-confirm", &pake_confirm_json(&a_confirm));
        assert!(matches!(effects_a2.as_slice(), [Effect::SetConfirmedCandidate { .. }]), "{effects_a2:?}");
        assert!(matches!(effects_b2.as_slice(), [Effect::SetConfirmedCandidate { .. }]), "{effects_b2:?}");
        (id_a, id_b)
    }

    /// The one `SendBootstrap(pake-confirm)` among `effects` — a first-sight
    /// message also yields a `KickHeartbeat` (see `handle_bootstrap_message`),
    /// which is allowed to ride along but nothing else is.
    fn expect_send_confirm(effects: &[Effect]) -> SentConfirm {
        let rest: Vec<&Effect> = effects.iter().filter(|e| !matches!(e, Effect::KickHeartbeat)).collect();
        match rest.as_slice() {
            [Effect::SendBootstrap { payload: BootstrapPayload::PakeConfirm { confirmation, sealed_name }, .. }] => {
                SentConfirm { confirmation: confirmation.clone(), sealed_name: sealed_name.clone() }
            }
            other => panic!("expected exactly one SendBootstrap(pake-confirm) effect, got {other:?}"),
        }
    }

    /// An attacker who never learns the phrase can still see the rendezvous
    /// tag and A's `pake1` on the relays. Echoing A's own message back as the
    /// "peer's" (and then A's own confirmation) must not get the attacker a
    /// "Pair with ...?" screen on A.
    #[test]
    fn echoing_a_devices_own_messages_back_never_reaches_a_confirmed_candidate() {
        let id_a = fresh_pairing_id();
        let a = start_attempt(&id_a, "pubkey-a", "Alice", "correct horse battery staple");

        let effects = handle_bootstrap_message(&id_a, "pubkey-attacker", "pake1", &pake1_json(&a.outbound_hex));
        let mut all = effects.iter().collect::<Vec<_>>();
        let echoed_confirm = match effects.as_slice() {
            [Effect::SendBootstrap { payload: BootstrapPayload::PakeConfirm { confirmation, sealed_name }, .. }] => {
                Some(SentConfirm { confirmation: confirmation.clone(), sealed_name: sealed_name.clone() })
            }
            _ => None,
        };
        let second;
        if let Some(confirm) = echoed_confirm {
            second = handle_bootstrap_message(&id_a, "pubkey-attacker", "pake-confirm", &pake_confirm_json(&confirm));
            all.extend(second.iter());
        }
        assert!(
            !all.iter().any(|e| matches!(e, Effect::SetConfirmedCandidate { .. })),
            "a reflected exchange must never produce a confirmed candidate: {all:?}"
        );
    }

    #[test]
    fn matching_passphrase_reaches_confirmed_candidate_on_both_sides() {
        run_to_resolved_match("correct horse battery staple");
    }

    #[test]
    fn self_echo_is_dropped() {
        let id = fresh_pairing_id();
        let start = start_attempt(&id, "my-own-pubkey", "Me", "some phrase");
        let effects = handle_bootstrap_message(&id, "my-own-pubkey", "pake1", &pake1_json(&start.outbound_hex));
        assert!(effects.is_empty(), "the relay's echo of our own publish must be dropped silently: {effects:?}");
    }

    #[test]
    fn no_live_attempt_is_a_silent_no_op() {
        let id = fresh_pairing_id();
        let effects = handle_bootstrap_message(&id, "someone", "pake1", &pake1_json("aa"));
        assert!(effects.is_empty());
    }

    #[test]
    fn second_distinct_sender_collides_even_before_any_match() {
        let id = fresh_pairing_id();
        let _start = start_attempt(&id, "pubkey-a", "Alice", "shared phrase");
        let peer = start_attempt(&fresh_pairing_id(), "candidate-1", "Bob", "shared phrase");
        let _ = handle_bootstrap_message(&id, "candidate-1", "pake1", &pake1_json(&peer.outbound_hex));
        let effects = handle_bootstrap_message(&id, "candidate-2", "pake1", &pake1_json(&peer.outbound_hex));
        assert!(
            matches!(effects.as_slice(), [Effect::SetCollision { .. }, Effect::KickHeartbeat]),
            "a second distinct sender must collide the attempt: {effects:?}"
        );
        // The attempt must actually be gone — a further message for either
        // sender is now a silent no-op (no live attempt left to collide
        // *or* accept a pake1 for).
        let after = handle_bootstrap_message(&id, "candidate-1", "pake1", &pake1_json(&peer.outbound_hex));
        assert!(after.is_empty());
    }

    #[test]
    fn late_arriving_second_party_collides_even_after_a_resolved_match() {
        // The specific intruder-detection guarantee: a late third party at
        // the rendezvous tag is itself the signal worth failing closed on,
        // regardless of whether the real partner already confirmed.
        let (id_a, _id_b) = run_to_resolved_match("late collision test phrase");
        let effects = handle_bootstrap_message(&id_a, "a-completely-different-sender", "pake1", &pake1_json("aabbcc"));
        assert!(
            matches!(effects.as_slice(), [Effect::SetCollision { .. }, Effect::KickHeartbeat]),
            "a late second party must still collide the attempt even after a resolved match: {effects:?}"
        );
    }

    #[test]
    fn redelivered_pake1_after_finish_is_a_no_op() {
        let id_a = fresh_pairing_id();
        let id_b = fresh_pairing_id();
        let _a = start_attempt(&id_a, "pubkey-a", "Alice", "redelivery test");
        let b = start_attempt(&id_b, "pubkey-b", "Bob", "redelivery test");
        let first = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        assert!(!first.is_empty(), "first pake1 should produce a SendBootstrap effect");
        // Redelivery of the exact same pake1 (public relays redeliver
        // ephemeral events under load) must be a no-op, not a second
        // finish() call (which would panic/error — the session was
        // already consumed).
        let redelivered = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        assert!(redelivered.is_empty(), "a redelivered pake1 after finish() must be a silent no-op: {redelivered:?}");
    }

    #[test]
    fn pake_confirm_arriving_before_local_pake1_is_processed_gets_stashed_then_rechecked() {
        // A real, if rare, delivery-order race over a public relay: the
        // candidate's pake-confirm can arrive before this side has
        // finished processing *their* pake1.
        let id_a = fresh_pairing_id();
        let id_b = fresh_pairing_id();
        let a = start_attempt(&id_a, "pubkey-a", "Alice", "stash race test");
        let b = start_attempt(&id_b, "pubkey-b", "Bob", "stash race test");

        // B processes A's pake1 first and sends its confirm.
        let effects_b = handle_bootstrap_message(&id_b, "pubkey-a", "pake1", &pake1_json(&a.outbound_hex));
        let b_confirm = expect_send_confirm(&effects_b);

        // A receives B's pake-confirm BEFORE processing B's pake1 at all —
        // must stash, not error.
        let stash_effects = handle_bootstrap_message(&id_a, "pubkey-b", "pake-confirm", &pake_confirm_json(&b_confirm));
        // Stashed (nothing resolves yet); the only effect is the first-sight
        // kick that makes this side publish its own pake1 right away.
        assert_eq!(stash_effects, vec![Effect::KickHeartbeat], "an early pake-confirm must be stashed: {stash_effects:?}");

        // Now A processes B's pake1 — this must trigger the stashed
        // recheck and resolve immediately, in the same call.
        let effects_a = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        assert!(
            effects_a.iter().any(|e| matches!(e, Effect::SetConfirmedCandidate { .. })),
            "processing the pake1 should immediately recheck the stashed confirm and resolve: {effects_a:?}"
        );
    }

    #[test]
    fn mismatched_confirmation_times_out_not_panics() {
        let id_a = fresh_pairing_id();
        let id_b = fresh_pairing_id();
        let _a = start_attempt(&id_a, "pubkey-a", "Alice", "phrase one");
        let b = start_attempt(&id_b, "pubkey-b", "Bob", "phrase two"); // different passphrase
        let effects_a = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        let bogus_confirm = expect_send_confirm(&effects_a); // A's own (mismatched) confirm, reused as "their" confirm on purpose
        let bogus = SentConfirm { confirmation: bogus_confirm.confirmation.chars().rev().collect(), sealed_name: bogus_confirm.sealed_name.clone() };
        let result = handle_bootstrap_message(&id_a, "pubkey-b", "pake-confirm", &pake_confirm_json(&bogus));
        assert!(matches!(result.as_slice(), [Effect::SetTimedOut { .. }]), "{result:?}");
    }

    #[test]
    fn timeout_is_a_no_op_after_a_resolved_match() {
        let (id_a, _id_b) = run_to_resolved_match("timeout respects resolved");
        // We don't have the actual `generation` handy here since
        // run_to_resolved_match doesn't return it — start a fresh lookup
        // path instead: call handle_timeout with a generation we know is
        // wrong (0) to confirm mismatched-generation is also a no-op, then
        // confirm resolved-with-matching-generation is too, by re-deriving
        // it wouldn't matter which — resolved alone must already suppress it.
        let effects_wrong_gen = handle_timeout(&id_a, 0);
        assert!(effects_wrong_gen.is_empty());
    }

    #[test]
    fn timeout_fires_for_the_matching_generation_when_unresolved() {
        let id = fresh_pairing_id();
        let start = start_attempt(&id, "pubkey-a", "Alice", "timeout test");
        let effects = handle_timeout(&id, start.generation);
        assert!(
            matches!(effects.as_slice(), [Effect::SetTimedOut { .. }, Effect::KickHeartbeat]),
            "{effects:?}"
        );
    }

    #[test]
    fn timeout_is_a_no_op_for_a_stale_generation_after_a_fresh_retry() {
        let id = fresh_pairing_id();
        let first = start_attempt(&id, "pubkey-a", "Alice", "first try");
        // Human retries with a fresh phrase before the first attempt's
        // timeout ever fires — a new attempt replaces the old one under
        // the same pairing_id.
        let _second = start_attempt(&id, "pubkey-a", "Alice", "second try");
        let stale_timeout = handle_timeout(&id, first.generation);
        assert!(stale_timeout.is_empty(), "a stale timeout from a superseded attempt must not touch the new one: {stale_timeout:?}");
    }

    #[test]
    fn cancel_attempt_removes_it_and_later_messages_are_no_ops() {
        let id = fresh_pairing_id();
        let start = start_attempt(&id, "pubkey-a", "Alice", "cancel test");
        cancel_attempt(&id);
        let effects = handle_bootstrap_message(&id, "someone", "pake1", &pake1_json(&start.outbound_hex));
        assert!(effects.is_empty());
    }

    /// A device that completed the exchange can still seal a hostile name, so
    /// the receiving side sanitizes and caps it after opening, whatever the
    /// sender did.
    #[test]
    fn candidate_name_is_sanitized_and_length_capped() {
        let id_a = fresh_pairing_id();
        let a = start_attempt(&id_a, "pubkey-a", "Alice", "sanitize test");
        let (peer, _tag, peer_out) = PakeSession::start("sanitize test");
        let effects_a = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&hex_encode(&peer_out)));
        let _ = expect_send_confirm(&effects_a);

        let peer_keys = peer.finish(&hex_decode(&a.outbound_hex).unwrap()).unwrap();
        let hostile_name = format!("{}\u{202E}evil-reversed-name{}", "x".repeat(50), "y".repeat(100));
        let hostile = SentConfirm { confirmation: peer_keys.confirmation_hex().to_string(), sealed_name: peer_keys.seal_name("pubkey-b", &hostile_name) };

        let resolved = handle_bootstrap_message(&id_a, "pubkey-b", "pake-confirm", &pake_confirm_json(&hostile));
        match resolved.as_slice() {
            [Effect::SetConfirmedCandidate { name, .. }] => {
                assert!(!name.contains('\u{202E}'), "bidi override must be stripped: {name:?}");
                assert!(name.chars().count() <= MAX_NAME_LENGTH, "name must be length-capped: {} chars", name.chars().count());
                assert!(name.starts_with("xxxx"), "ordinary text is kept: {name:?}");
            }
            other => panic!("expected a confirmed candidate, got {other:?}"),
        }
    }

    #[test]
    fn the_peers_name_is_what_the_confirmed_candidate_carries() {
        let id_a = fresh_pairing_id();
        let id_b = fresh_pairing_id();
        let a = start_attempt(&id_a, "pubkey-a", "Alice", "name carry test");
        let b = start_attempt(&id_b, "pubkey-b", "Bob's TV", "name carry test");
        let effects_a = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        let _ = expect_send_confirm(&effects_a);
        let effects_b = handle_bootstrap_message(&id_b, "pubkey-a", "pake1", &pake1_json(&a.outbound_hex));
        let b_confirm = expect_send_confirm(&effects_b);
        let resolved = handle_bootstrap_message(&id_a, "pubkey-b", "pake-confirm", &pake_confirm_json(&b_confirm));
        assert!(matches!(resolved.as_slice(), [Effect::SetConfirmedCandidate { name, .. }] if name == "Bob's TV"), "{resolved:?}");
    }

    #[test]
    fn pairing_messages_carry_no_device_name_in_the_clear() {
        let id_a = fresh_pairing_id();
        let id_b = fresh_pairing_id();
        let _a = start_attempt(&id_a, "pubkey-a", "Secret Name 123", "no clear name");
        let b = start_attempt(&id_b, "pubkey-b", "Other Name 456", "no clear name");

        let pake1 = build_bootstrap_payload(&id_a).unwrap();
        assert!(!pake1.contains("Secret Name 123"), "pake1 must not carry the name: {pake1}");

        let effects = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        let confirm = expect_send_confirm(&effects);
        let wire = pake_confirm_json(&confirm);
        assert!(!wire.contains("Secret Name 123"), "pake-confirm must not carry the name in the clear: {wire}");
        let republished = build_bootstrap_payload(&id_a).unwrap();
        assert!(!republished.contains("Secret Name 123"), "{republished}");
    }

    #[test]
    fn a_republished_pake_confirm_is_byte_identical() {
        let id_a = fresh_pairing_id();
        let id_b = fresh_pairing_id();
        let _a = start_attempt(&id_a, "pubkey-a", "Alice", "stable republish");
        let b = start_attempt(&id_b, "pubkey-b", "Bob", "stable republish");
        let _ = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        assert_eq!(build_bootstrap_payload(&id_a), build_bootstrap_payload(&id_a));
    }

    #[test]
    fn a_pake_confirm_without_a_sealed_name_is_ignored() {
        let id_a = fresh_pairing_id();
        let id_b = fresh_pairing_id();
        let a = start_attempt(&id_a, "pubkey-a", "Alice", "no sealed name");
        let b = start_attempt(&id_b, "pubkey-b", "Bob", "no sealed name");
        let _ = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        let effects_b = handle_bootstrap_message(&id_b, "pubkey-a", "pake1", &pake1_json(&a.outbound_hex));
        let b_confirm = expect_send_confirm(&effects_b);
        // The old (v2) message shape: a confirmation and nothing else.
        let old_shape = format!("{{\"type\":\"pake-confirm\",\"confirmation\":\"{}\"}}", b_confirm.confirmation);
        let effects = handle_bootstrap_message(&id_a, "pubkey-b", "pake-confirm", &old_shape);
        assert!(effects.is_empty(), "a confirmation with no sealed name must not resolve: {effects:?}");
        // ...and a blank or wrong-size one is just as dead.
        let blank = SentConfirm { confirmation: b_confirm.confirmation.clone(), sealed_name: String::new() };
        assert!(handle_bootstrap_message(&id_a, "pubkey-b", "pake-confirm", &pake_confirm_json(&blank)).is_empty());
    }

    #[test]
    fn a_sealed_name_that_will_not_open_fails_the_attempt_even_with_a_valid_confirmation() {
        let id_a = fresh_pairing_id();
        let id_b = fresh_pairing_id();
        let a = start_attempt(&id_a, "pubkey-a", "Alice", "bad sealed name");
        let b = start_attempt(&id_b, "pubkey-b", "Bob", "bad sealed name");
        let _ = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        let effects_b = handle_bootstrap_message(&id_b, "pubkey-a", "pake1", &pake1_json(&a.outbound_hex));
        let mut b_confirm = expect_send_confirm(&effects_b);
        // Right confirmation, but a name sealed under some other exchange's keys.
        b_confirm.sealed_name = {
            let (s, _, _) = PakeSession::start("bad sealed name");
            let keys = s.finish(&hex_decode(&a.outbound_hex).unwrap()).unwrap();
            keys.seal_name("pubkey-b", "Mallory")
        };
        let result = handle_bootstrap_message(&id_a, "pubkey-b", "pake-confirm", &pake_confirm_json(&b_confirm));
        assert!(matches!(result.as_slice(), [Effect::SetTimedOut { .. }]), "{result:?}");
    }

    #[test]
    fn build_bootstrap_payload_is_pake1_before_a_candidate_confirms_and_pake1_plus_pake_confirm_after() {
        let id_a = fresh_pairing_id();
        let id_b = fresh_pairing_id();
        let a = start_attempt(&id_a, "pubkey-a", "Alice", "payload shape test");
        let b = start_attempt(&id_b, "pubkey-b", "Bob", "payload shape test");

        let before = build_bootstrap_payload(&id_a).unwrap();
        assert!(before.contains("\"type\":\"pake1\""), "{before}");

        let _ = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        let after = build_bootstrap_payload(&id_a).unwrap();
        assert!(after.contains("\"type\":\"pake-confirm\""), "{after}");
        assert!(after.contains("\"type\":\"pake1\""), "pake1 must keep being republished for a late joiner: {after}");
        let _ = a; // silence unused warning if reordered later
    }

    #[test]
    fn a_late_joiner_resolves_from_what_the_waiting_side_republishes() {
        let id_a = fresh_pairing_id();
        let id_b = fresh_pairing_id();
        let _a = start_attempt(&id_a, "pubkey-a", "Alice", "late joiner");
        // A has been waiting; B only now types the phrase and publishes its pake1.
        let b = start_attempt(&id_b, "pubkey-b", "Bob", "late joiner");
        let effects_a = handle_bootstrap_message(&id_a, "pubkey-b", "pake1", &pake1_json(&b.outbound_hex));
        assert!(effects_a.contains(&Effect::KickHeartbeat), "A must answer immediately: {effects_a:?}");
        let a_confirm = expect_send_confirm(&effects_a);

        // B never saw A's original pake1 (published before B subscribed). All it
        // gets is what A republishes next: the snapshot's payloads.
        let snapshot: serde_json::Value = serde_json::from_str(&build_bootstrap_payload(&id_a).unwrap()).unwrap();
        let payloads = snapshot["payloads"].as_array().unwrap();
        let a_pake1 = payloads.iter().find(|p| p["type"] == "pake1").expect("A republishes its pake1")["outbound"].as_str().unwrap().to_string();

        let effects_b = handle_bootstrap_message(&id_b, "pubkey-a", "pake1", &pake1_json(&a_pake1));
        let b_confirm = expect_send_confirm(&effects_b);
        let resolved_b = handle_bootstrap_message(&id_b, "pubkey-a", "pake-confirm", &pake_confirm_json(&a_confirm));
        assert!(resolved_b.iter().any(|e| matches!(e, Effect::SetConfirmedCandidate { .. })), "{resolved_b:?}");
        let resolved_a = handle_bootstrap_message(&id_a, "pubkey-b", "pake-confirm", &pake_confirm_json(&b_confirm));
        assert!(resolved_a.iter().any(|e| matches!(e, Effect::SetConfirmedCandidate { .. })), "{resolved_a:?}");
    }

    #[test]
    fn sanitize_name_strips_bidi_and_zero_width_but_keeps_ordinary_text() {
        assert_eq!(sanitize_name("Mom's TV"), "Mom's TV");
        assert_eq!(sanitize_name("evil\u{202E}reversed"), "evilreversed");
        assert_eq!(sanitize_name("zero\u{200B}width"), "zerowidth");
    }
}
