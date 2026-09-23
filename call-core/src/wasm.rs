//! `wasm-bindgen` glue for the web client. Unlike `pake-bridge`'s
//! WASM-side `PakeSession` class (needed there because a session has real
//! per-call lifecycle), every function here is a plain string-in/
//! string-out call — `call-core` owns all attempt state internally, keyed
//! by `pairing_id` (see `lib.rs`'s own doc), so there's no session object
//! for JS to hold onto at all. Structured values cross as JSON; `app.js`'s
//! own dispatch loop `JSON.parse`s them.
//!
//! `generation` is typed `u32` at this boundary specifically (not the
//! `u64` it is internally) purely for JS ergonomics — `u64` would require
//! `BigInt` on the JS side via wasm-bindgen's default numeric mapping, and
//! a per-process monotonic counter will never realistically approach
//! 2^32, let alone need to cross it — a plain JS `number` is fine.
//!
//! **Every exported function wraps its body in [`catch_unwind`] and
//! returns the same fail-closed sentinel `android.rs`'s equivalent JNI
//! export already uses on panic**: this crate's input ultimately comes
//! from whatever arrived over a public relay, not trusted, well-formed
//! data. Without this, `wasm32-unknown-unknown` has no unwind support and
//! this project registers no panic hook, so a single panic anywhere in
//! this crate's dependency chain — triggered by a malformed or adversarial
//! relay event — propagates as an uncaught, uncatchable trap that leaves
//! the WASM instance permanently unusable for the rest of that page load,
//! unlike Android's graceful fail-closed degrade. See each function's own
//! doc for its specific fallback value, chosen to match `android.rs`'s
//! existing convention exactly rather than invent a second one.

use std::panic::catch_unwind;
use wasm_bindgen::prelude::*;

fn empty_effects_json() -> String {
    "[]".to_string()
}

fn empty_presence_result_json() -> String {
    "{\"presence_effects\":[],\"call_effects\":[]}".to_string()
}

// ---------------------------------------------------------------------------
// `universal-time` provider — required at link time on `wasm32-unknown-
// unknown` specifically: no working `std::time` exists for this target,
// unlike other WASM targets that keep a host runtime underneath.
// `nostr::Timestamp::now()` (called internally by
// `EventBuilder::finalize_unsigned` whenever an event is built without an
// explicit `custom_created_at` — i.e. every event this crate builds) needs
// this satisfied to link at all, whether or not that particular code path
// is ever actually exercised at runtime: the reference is unconditional in
// `finalize_unsigned`'s own compiled body. Backed by `js_sys::Date::now()`,
// the only wall-clock source available in a browser; reused for the
// monotonic half too since nothing in this crate relies on genuine
// monotonicity, only on `Timestamp::now()` producing a plausible-enough
// Unix time for an event's `created_at` field.
// ---------------------------------------------------------------------------

struct JsDateTimeProvider;

impl universal_time::WallClock for JsDateTimeProvider {
    fn system_time(&self) -> universal_time::SystemTime {
        let millis_since_epoch = js_sys::Date::now();
        universal_time::SystemTime::from_unix_duration(std::time::Duration::from_millis(millis_since_epoch as u64))
    }
}

impl universal_time::MonotonicClock for JsDateTimeProvider {
    fn instant(&self) -> universal_time::Instant {
        let millis = js_sys::Date::now();
        universal_time::Instant::from_ticks(std::time::Duration::from_millis(millis as u64))
    }
}

universal_time::define_time_provider!(JsDateTimeProvider);

/// See [`crate::start_attempt`]'s own doc. Returns a JSON-encoded
/// [`crate::StartResult`], or a JS exception on a panic — matches
/// `nativeStartAttempt`'s `null`-on-panic in spirit (both are this
/// function's existing "something went wrong, nothing to hand back"
/// signal); an exception here is easy for `app.js` to `.catch()` the same
/// way it already handles this function's genuine `Err` path.
#[wasm_bindgen(js_name = startAttempt)]
pub fn start_attempt(pairing_id: &str, own_pubkey_hex: &str, own_name: &str, passphrase: &str) -> Result<String, JsValue> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = crate::start_attempt(pairing_id, own_pubkey_hex, own_name, passphrase);
        serde_json::to_string(&result).map_err(|e| JsValue::from_str(&e.to_string()))
    }))
    .unwrap_or_else(|_| Err(JsValue::from_str("call-core panicked in startAttempt")))
}

/// See [`crate::cancel_attempt`]'s own doc. No return value — a panic here
/// is swallowed the same way `nativeCancelAttempt` swallows one.
#[wasm_bindgen(js_name = cancelAttempt)]
pub fn cancel_attempt(pairing_id: &str) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::cancel_attempt(pairing_id)));
}

/// See [`crate::build_bootstrap_payload`]'s own doc. Returns `undefined`
/// (not an empty string) if there's no live attempt, matching that
/// function's own `Option` return in a way `app.js` can check with a plain
/// falsy test — also the fallback on a panic, same sentinel either way,
/// matching `nativeBuildBootstrapPayload`'s own convention.
#[wasm_bindgen(js_name = buildBootstrapPayload)]
pub fn build_bootstrap_payload(pairing_id: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::build_bootstrap_payload(pairing_id))).ok().flatten()
}

/// See [`crate::handle_bootstrap_message`]'s own doc. Returns a
/// JSON-encoded array of [`crate::Effect`] — always a valid JSON array
/// (possibly empty, `[]`), never `undefined`, including on a panic.
#[wasm_bindgen(js_name = handleBootstrapMessage)]
pub fn handle_bootstrap_message(pairing_id: &str, sender_pubkey: &str, type_: &str, payload_json: &str) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let effects = crate::handle_bootstrap_message(pairing_id, sender_pubkey, type_, payload_json);
        serde_json::to_string(&effects).unwrap_or_else(|_| empty_effects_json())
    }))
    .unwrap_or_else(|_| empty_effects_json())
}

/// See [`crate::handle_timeout`]'s own doc. `generation` must be exactly
/// the value returned in `startAttempt`'s JSON result (see this module's
/// own doc for why it's `u32` here, not `u64`).
#[wasm_bindgen(js_name = handleTimeout)]
pub fn handle_timeout(pairing_id: &str, generation: u32) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let effects = crate::handle_timeout(pairing_id, generation as u64);
        serde_json::to_string(&effects).unwrap_or_else(|_| empty_effects_json())
    }))
    .unwrap_or_else(|_| empty_effects_json())
}

/// See [`crate::sanitize_name`]'s own doc — exposed standalone too, so
/// `app.js`'s own `sanitizeName` (used for own-device name entry/rename,
/// not just the pairing flow) can be replaced by this single
/// implementation instead of keeping a second hand-copy around. Returns
/// the original string unchanged on a panic (fails open, deliberately,
/// unlike every other function in this file) — matches
/// `nativeSanitizeName`'s own identical reasoning: this is display
/// sanitization, not a security boundary on its own, and losing a human's
/// typed name entirely on an internal panic is worse than showing it
/// unsanitized.
#[wasm_bindgen(js_name = sanitizeName)]
pub fn sanitize_name(name: &str) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::sanitize_name(name))).unwrap_or_else(|_| name.to_string())
}

// ---------------------------------------------------------------------------
// Call arbitration — see `crate::call_arbitration`'s own doc for the full
// design. Unlike the JNI bindings above (which encode a null `sdpMid` as an
// empty string to sidestep JNI's own null-JString handling), `wasm-bindgen`
// supports `Option<String>` parameters directly — `null`/`undefined` from
// JS map straight to `None`, no sentinel needed.
// ---------------------------------------------------------------------------

/// See [`crate::call_arbitration::request_call`]'s own doc. Returns a
/// JSON-encoded [`crate::call_arbitration::RequestCallResult`], or a JS
/// exception on a panic (see [`start_attempt`]'s own doc for why that's
/// the right shape here, not a silent fallback value — this function's
/// only reachable from a local, already-validated UI action, never
/// directly from relay input).
#[wasm_bindgen(js_name = requestCall)]
pub fn request_call(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, peer_online: bool) -> Result<String, JsValue> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = crate::call_arbitration::request_call(pairing_id, own_pubkey_hex, peer_pubkey_hex, peer_online);
        serde_json::to_string(&result).map_err(|e| JsValue::from_str(&e.to_string()))
    }))
    .unwrap_or_else(|_| Err(JsValue::from_str("call-core panicked in requestCall")))
}

/// See [`crate::call_arbitration::handle_should_offer`]'s own doc — the
/// pubkey tie-break lives inside that function itself, so the shell just
/// forwards both pubkeys through unconditionally like every other message
/// type. `auto_answer` isn't a web feature (see `handleOffer`'s own doc/
/// `onOfferReceived`'s call site) — `app.js` always passes `false` here
/// too, for the same reason.
#[wasm_bindgen(js_name = handleShouldOffer)]
pub fn handle_should_offer(pairing_id: &str, call_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, auto_answer: bool) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let effects = crate::call_arbitration::handle_should_offer(pairing_id, call_id, own_pubkey_hex, peer_pubkey_hex, auto_answer);
        serde_json::to_string(&effects).unwrap_or_else(|_| empty_effects_json())
    }))
    .unwrap_or_else(|_| empty_effects_json())
}

/// See [`crate::call_arbitration::handle_offer`]'s own doc — no longer
/// takes a `has_active_peer_connection` flag (dropped from the Rust
/// signature once found to be provably redundant with `offer_applied`).
#[wasm_bindgen(js_name = handleOffer)]
pub fn handle_offer(pairing_id: &str, call_id: &str, sdp: &str, auto_answer: bool) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let effects = crate::call_arbitration::handle_offer(pairing_id, call_id, sdp, auto_answer);
        serde_json::to_string(&effects).unwrap_or_else(|_| empty_effects_json())
    }))
    .unwrap_or_else(|_| empty_effects_json())
}

/// See [`crate::call_arbitration::should_apply_answer`]'s own doc. Returns
/// `false` on a panic, matching `nativeShouldApplyAnswer`'s own fail-closed
/// default (don't apply an answer this module couldn't safely reason
/// about).
#[wasm_bindgen(js_name = shouldApplyAnswer)]
pub fn should_apply_answer(pairing_id: &str, call_id: &str) -> bool {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::call_arbitration::should_apply_answer(pairing_id, call_id))).unwrap_or(false)
}

/// See [`crate::call_arbitration::handle_remote_ice`]'s own doc. Returns a
/// JSON-encoded [`crate::call_arbitration::IceOutcome`], falling back to
/// `Dropped` on a panic — matches `nativeHandleRemoteIce`'s own reasoning
/// (silently discarding one ICE candidate is harmless, trickle ICE sends
/// several; applying/buffering one from state that couldn't even be read
/// safely is not worth the risk).
#[wasm_bindgen(js_name = handleRemoteIce)]
pub fn handle_remote_ice(pairing_id: &str, call_id: &str, sdp_mid: Option<String>, sdp_m_line_index: i32, candidate: &str) -> Result<String, JsValue> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let outcome = crate::call_arbitration::handle_remote_ice(pairing_id, call_id, sdp_mid.as_deref(), sdp_m_line_index, candidate);
        serde_json::to_string(&outcome).map_err(|e| JsValue::from_str(&e.to_string()))
    }))
    .unwrap_or_else(|_| Ok("{\"outcome\":\"Dropped\"}".to_string()))
}

/// See [`crate::call_arbitration::accept_incoming_call`]'s own doc. Returns
/// a JSON-encoded, tagged [`crate::call_arbitration::AcceptOutcome`] (`kind:
/// "ApplyOffer" | "CreateOffer"`), or `undefined` (not an empty string) if
/// there's nothing pending, matching [`build_bootstrap_payload`]'s own
/// `Option` convention — also the fallback on a panic, same sentinel either
/// way, matching `nativeAcceptIncomingCall`'s own convention.
#[wasm_bindgen(js_name = acceptIncomingCall)]
pub fn accept_incoming_call() -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = crate::call_arbitration::accept_incoming_call()?;
        serde_json::to_string(&result).ok()
    }))
    .ok()
    .flatten()
}

/// See [`crate::call_arbitration::tick_incoming_call_countdown`]'s own doc.
/// Returns a JSON-encoded [`crate::call_arbitration::TickOutcome`], falling
/// back to `Stale` on a panic — matches `nativeTickIncomingCallCountdown`'s
/// own reasoning (a countdown that silently stops rather than one that
/// might auto-accept a call it couldn't safely reason about).
#[wasm_bindgen(js_name = tickIncomingCallCountdown)]
pub fn tick_incoming_call_countdown(pairing_id: &str, call_id: &str) -> Result<String, JsValue> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let outcome = crate::call_arbitration::tick_incoming_call_countdown(pairing_id, call_id);
        serde_json::to_string(&outcome).map_err(|e| JsValue::from_str(&e.to_string()))
    }))
    .unwrap_or_else(|_| Ok("{\"outcome\":\"Stale\"}".to_string()))
}

/// See [`crate::call_arbitration::hang_up`]'s own doc. Falls back to just
/// `[ClosePeerConnection]` on a panic — matches `nativeHangUp`'s own
/// reasoning: even if this module's own state couldn't be safely
/// read/cleared, the shell must still be told to tear down the real
/// `PeerConnection`, or a live call would leak.
#[wasm_bindgen(js_name = hangUp)]
pub fn hang_up() -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let effects = crate::call_arbitration::hang_up();
        serde_json::to_string(&effects).unwrap_or_else(|_| empty_effects_json())
    }))
    .unwrap_or_else(|_| "[{\"kind\":\"ClosePeerConnection\"}]".to_string())
}

/// See [`crate::call_arbitration::handle_peer_hangup`]'s own doc.
#[wasm_bindgen(js_name = handlePeerHangup)]
pub fn handle_peer_hangup(pairing_id: &str, call_id: &str) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let effects = crate::call_arbitration::handle_peer_hangup(pairing_id, call_id);
        serde_json::to_string(&effects).unwrap_or_else(|_| empty_effects_json())
    }))
    .unwrap_or_else(|_| empty_effects_json())
}

/// See [`crate::call_arbitration::handle_peer_busy`]'s own doc.
#[wasm_bindgen(js_name = handlePeerBusy)]
pub fn handle_peer_busy(pairing_id: &str, call_id: &str) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let effects = crate::call_arbitration::handle_peer_busy(pairing_id, call_id);
        serde_json::to_string(&effects).unwrap_or_else(|_| empty_effects_json())
    }))
    .unwrap_or_else(|_| empty_effects_json())
}

/// See [`crate::call_arbitration::peer_connection_closed`]'s own doc.
/// **Now returns a JSON-encoded effects array** (was `void`) — see that
/// function's own doc for why (a genuinely spontaneous WebRTC teardown can
/// now carry a `ShowCallOutcome` down to the shell).
#[wasm_bindgen(js_name = peerConnectionClosed)]
pub fn peer_connection_closed() -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let effects = crate::call_arbitration::peer_connection_closed();
        serde_json::to_string(&effects).unwrap_or_else(|_| empty_effects_json())
    }))
    .unwrap_or_else(|_| empty_effects_json())
}

/// See [`crate::call_arbitration::mark_connected`]'s own doc. No return
/// value — pure bookkeeping, a panic here is swallowed the same way
/// `nativeMarkConnected` swallows one.
#[wasm_bindgen(js_name = markConnected)]
pub fn mark_connected(pairing_id: &str, call_id: &str) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::call_arbitration::mark_connected(pairing_id, call_id)));
}

/// See [`crate::call_arbitration::should_end_call_on_media_failure`]'s own
/// doc. Falls back to `true` (end the call) on a panic — matches
/// `nativeShouldEndCallOnMediaFailure`'s own reasoning: the safer
/// direction when this module's own logic couldn't be trusted, since a
/// call wrongly ended is recoverable (redial) but a media failure wrongly
/// treated as harmless during an actual live call is a worse experience (a
/// dead call that looks alive).
#[wasm_bindgen(js_name = shouldEndCallOnMediaFailure)]
pub fn should_end_call_on_media_failure(has_active_peer_connection: bool) -> bool {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::call_arbitration::should_end_call_on_media_failure(has_active_peer_connection))).unwrap_or(true)
}

/// See [`crate::call_arbitration::forget_pairing`]'s own doc. Same
/// always-a-JSON-array contract as the other effect-returning functions
/// above.
#[wasm_bindgen(js_name = forgetPairing)]
pub fn forget_pairing(pairing_id: &str) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let effects = crate::call_arbitration::forget_pairing(pairing_id);
        serde_json::to_string(&effects).unwrap_or_else(|_| empty_effects_json())
    }))
    .unwrap_or_else(|_| empty_effects_json())
}

// ---------------------------------------------------------------------------
// Presence — see `crate::presence`'s own doc for the full design.
// `Vec<String>` parameters (`pending_pairing_ids`) cross directly as a JS
// array of strings — `wasm-bindgen` supports `Vec<String>` natively, unlike
// the JNI side above which has to fall back to a JSON-encoded array.
// ---------------------------------------------------------------------------

/// See [`crate::presence::mark_seen`]'s own doc. Returns a JSON-encoded
/// [`crate::presence::PresenceUpdateResult`], falling back to an empty one
/// on a panic (fail-closed: no UI transition, no call effect — matches
/// `nativeMarkSeen`'s own reasoning) — `now_ms` is `f64` (JS's own
/// `Date.now()` return type — `wasm-bindgen` maps `i64` to `BigInt`, which
/// `app.js` doesn't otherwise need to deal with anywhere else in this
/// module). `peer_busy`: `Option<bool>` crosses directly (`null`/`undefined`
/// from JS map straight to `None`, no JNI-style sentinel needed here) — see
/// `mark_seen`'s own doc for why this is `Some(_)` only for a `"heartbeat"`
/// message.
#[wasm_bindgen(js_name = markSeen)]
pub fn mark_seen(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, now_ms: f64, peer_busy: Option<bool>) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let update = crate::presence::mark_seen(pairing_id, own_pubkey_hex, peer_pubkey_hex, now_ms as i64, peer_busy);
        serde_json::to_string(&update).unwrap_or_else(|_| empty_presence_result_json())
    }))
    .unwrap_or_else(|_| empty_presence_result_json())
}

/// See [`crate::presence::handle_peer_busy_reply`]'s own doc. Falls back to
/// an empty result on a panic, same reasoning as [`mark_seen`] above. Takes
/// the same `own_pubkey_hex`/`peer_pubkey_hex` tie-break inputs [`mark_seen`]
/// does — this function now performs its own online transition rather than
/// relying on the caller having already called `markSeen` first, so it
/// needs the same two pieces of context that transition requires.
#[wasm_bindgen(js_name = handlePeerBusyReply)]
pub fn handle_peer_busy_reply(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, call_id: &str) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let update = crate::presence::handle_peer_busy_reply(pairing_id, own_pubkey_hex, peer_pubkey_hex, call_id);
        serde_json::to_string(&update).unwrap_or_else(|_| empty_presence_result_json())
    }))
    .unwrap_or_else(|_| empty_presence_result_json())
}

/// See [`crate::presence::is_peer_busy`]'s own doc. Returns `false` on a
/// panic, matching [`is_online`]'s own "never seen" sentinel.
#[wasm_bindgen(js_name = isPeerBusy)]
pub fn is_peer_busy(pairing_id: &str) -> bool {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::presence::is_peer_busy(pairing_id))).unwrap_or(false)
}

/// See [`crate::call_arbitration::is_call_active`]'s own doc. Returns
/// `true` (fail toward *not* claiming to be free) on a panic — the shell
/// uses this to fill in its own outgoing heartbeat's busy field, and
/// wrongly broadcasting "not busy" while this module's own logic couldn't
/// be trusted is the worse direction to fail in.
#[wasm_bindgen(js_name = isCallActive)]
pub fn is_call_active() -> bool {
    catch_unwind(std::panic::AssertUnwindSafe(crate::call_arbitration::is_call_active)).unwrap_or(true)
}

/// See [`crate::presence::handle_leaving_message`]'s own doc.
#[wasm_bindgen(js_name = handleLeavingMessage)]
pub fn handle_leaving_message(pairing_id: &str) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let update = crate::presence::handle_leaving_message(pairing_id);
        serde_json::to_string(&update).unwrap_or_else(|_| empty_presence_result_json())
    }))
    .unwrap_or_else(|_| empty_presence_result_json())
}

/// See [`crate::presence::check_online_timeouts`]'s own doc. `now_ms`: see
/// [`mark_seen`]'s own doc for why `f64`, not `i64`.
#[wasm_bindgen(js_name = checkOnlineTimeouts)]
pub fn check_online_timeouts(now_ms: f64) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let update = crate::presence::check_online_timeouts(now_ms as i64);
        serde_json::to_string(&update).unwrap_or_else(|_| empty_presence_result_json())
    }))
    .unwrap_or_else(|_| empty_presence_result_json())
}

/// See [`crate::presence::is_online`]'s own doc. Returns `false` on a
/// panic, matching `nativeIsOnline`'s own "never seen" sentinel.
#[wasm_bindgen(js_name = isOnline)]
pub fn is_online(pairing_id: &str) -> bool {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::presence::is_online(pairing_id))).unwrap_or(false)
}

/// See [`crate::presence::current_heartbeat_interval_ms`]'s own doc.
/// `now_ms`: see [`mark_seen`]'s own doc for why `f64`. Returns `u32`
/// directly (a plain JS `number`) — small enough this module doesn't need
/// the `handleTimeout`-style `u32`-at-the-boundary workaround, it just is
/// one already. Falls back to [`crate::presence::HEARTBEAT_INTERVAL_MS`]
/// (the slow, steady-state cadence) on a panic, matching
/// `nativeCurrentHeartbeatIntervalMs`'s own fail-closed choice — a
/// performance detail, not a correctness one.
#[wasm_bindgen(js_name = currentHeartbeatIntervalMs)]
pub fn current_heartbeat_interval_ms(pending_pairing_ids: Vec<String>, now_ms: f64) -> u32 {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::presence::current_heartbeat_interval_ms(&pending_pairing_ids, now_ms as i64)))
        .unwrap_or(crate::presence::HEARTBEAT_INTERVAL_MS)
}

/// See [`crate::presence::prune_stale_pending`]'s own doc. No return value;
/// a panic is a silent no-op (the map just doesn't get pruned this tick —
/// it'll be pruned next time, same as any other missed tick, matching
/// `nativePruneStalePending`'s own reasoning).
#[wasm_bindgen(js_name = pruneStalePending)]
pub fn prune_stale_pending(current_pending_ids: Vec<String>) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::presence::prune_stale_pending(&current_pending_ids)));
}

/// See [`crate::presence::remove_pairing`]'s own doc. No return value — a
/// panic here is swallowed the same way `nativePresenceRemovePairing`
/// swallows one.
#[wasm_bindgen(js_name = presenceRemovePairing)]
pub fn presence_remove_pairing(pairing_id: &str) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::presence::remove_pairing(pairing_id)));
}

// ---------------------------------------------------------------------------
// Nostr protocol — see `crate::nostr_protocol`'s own doc for the full
// design. `Vec<String>` parameters cross directly as a JS array of
// strings, same as the presence bindings above; `Option<String>`
// (`target_pubkey_hex`) crosses directly too — no JNI-style empty-string
// sentinel needed here, `wasm-bindgen` maps `null`/`undefined` straight to
// `None`.
// ---------------------------------------------------------------------------

/// See [`crate::nostr_protocol::build_wrapped_event`]'s own doc. Returns
/// the signed outer event as JSON, or `undefined` on any failure —
/// including a panic, matching `nativeBuildWrappedEvent`'s own convention.
#[wasm_bindgen(js_name = buildWrappedEvent)]
pub fn build_wrapped_event(own_private_key_hex: &str, target_pubkey_hex: &str, payload_json: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::nostr_protocol::build_wrapped_event(own_private_key_hex, target_pubkey_hex, payload_json)))
        .ok()
        .flatten()
}

/// See [`crate::nostr_protocol::unwrap_wrapped_event_for_any`]'s own doc.
/// `candidates_json` is a JSON array of
/// [`crate::nostr_protocol::WrapEventCandidate`] (this client's confirmed
/// peers — `pairing_id`/`own_private_key_hex`/`peer_public_key` each).
/// Returns a JSON-encoded [`crate::nostr_protocol::RoutedSignalPayload`],
/// or `undefined` if the event doesn't route to any of `candidates`, fails
/// to decrypt/verify once routed, is malformed, or on a panic — same
/// fail-closed posture as every other "a confirmed peer's message this
/// client can't safely interpret" case in this file.
#[wasm_bindgen(js_name = unwrapWrappedEventForAny)]
pub fn unwrap_wrapped_event_for_any(wrap_event_json: &str, candidates_json: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let candidates: Vec<crate::nostr_protocol::WrapEventCandidate> = serde_json::from_str(candidates_json).ok()?;
        let routed = crate::nostr_protocol::unwrap_wrapped_event_for_any(wrap_event_json, &candidates)?;
        serde_json::to_string(&routed).ok()
    }))
    .ok()
    .flatten()
}

/// See [`crate::nostr_protocol::build_bootstrap_event`]'s own doc.
#[wasm_bindgen(js_name = buildBootstrapEvent)]
pub fn build_bootstrap_event(own_private_key_hex: &str, rendezvous_tag: &str, target_pubkey_hex: Option<String>, payload_json: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::nostr_protocol::build_bootstrap_event(own_private_key_hex, rendezvous_tag, target_pubkey_hex.as_deref(), payload_json)
    }))
    .ok()
    .flatten()
}

/// See [`crate::nostr_protocol::verify_bootstrap_event`]'s own doc.
/// Returns a JSON-encoded [`crate::nostr_protocol::VerifiedBootstrapEvent`],
/// or `undefined` if verification fails or on a panic — same sentinel
/// either way, matching `nativeVerifyBootstrapEvent`'s own convention.
#[wasm_bindgen(js_name = verifyBootstrapEvent)]
pub fn verify_bootstrap_event(event_json: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let verified = crate::nostr_protocol::verify_bootstrap_event(event_json)?;
        serde_json::to_string(&verified).ok()
    }))
    .ok()
    .flatten()
}

/// See [`crate::nostr_protocol::mark_seen_or_is_duplicate`]'s own doc.
/// Returns `true` (treat as a duplicate, i.e. drop it) on a panic, matching
/// `nativeMarkSeenOrIsDuplicate`'s own fail-closed default — safer to
/// silently drop one relay-redelivered event than to risk reprocessing
/// something this module couldn't safely reason about.
#[wasm_bindgen(js_name = markSeenOrIsDuplicate)]
pub fn mark_seen_or_is_duplicate(event_id: &str) -> bool {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::nostr_protocol::mark_seen_or_is_duplicate(event_id))).unwrap_or(true)
}

/// See [`crate::nostr_protocol::build_relay_filters`]'s own doc. Returns a
/// JSON-encoded [`crate::nostr_protocol::RelayFilters`], falling back to
/// `{"wrap_filter":null,"bootstrap_filter":null}` on a panic — same
/// "nothing to filter for" no-op `nativeBuildRelayFilters` falls back to.
#[wasm_bindgen(js_name = buildRelayFilters)]
pub fn build_relay_filters(confirmed_own_pubkeys: Vec<String>, pending_rendezvous_tags: Vec<String>) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let filters = crate::nostr_protocol::build_relay_filters(&confirmed_own_pubkeys, &pending_rendezvous_tags);
        serde_json::to_string(&filters).unwrap_or_else(|_| "{\"wrap_filter\":null,\"bootstrap_filter\":null}".to_string())
    }))
    .unwrap_or_else(|_| "{\"wrap_filter\":null,\"bootstrap_filter\":null}".to_string())
}

/// See [`crate::nostr_protocol::build_heartbeat_payload`]'s own doc.
#[wasm_bindgen(js_name = buildHeartbeatPayload)]
pub fn build_heartbeat_payload(name: &str, busy: bool) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::nostr_protocol::build_heartbeat_payload(name, busy))).ok().flatten()
}

/// See [`crate::nostr_protocol::build_leaving_payload`]'s own doc.
#[wasm_bindgen(js_name = buildLeavingPayload)]
pub fn build_leaving_payload() -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(crate::nostr_protocol::build_leaving_payload)).ok().flatten()
}

/// See [`crate::nostr_protocol::build_bye_payload`]'s own doc.
#[wasm_bindgen(js_name = buildByePayload)]
pub fn build_bye_payload(call_id: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::nostr_protocol::build_bye_payload(call_id))).ok().flatten()
}

/// See [`crate::nostr_protocol::build_busy_payload`]'s own doc.
#[wasm_bindgen(js_name = buildBusyPayload)]
pub fn build_busy_payload(call_id: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::nostr_protocol::build_busy_payload(call_id))).ok().flatten()
}

/// See [`crate::nostr_protocol::build_call_payload`]'s own doc.
#[wasm_bindgen(js_name = buildCallPayload)]
pub fn build_call_payload(call_id: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::nostr_protocol::build_call_payload(call_id))).ok().flatten()
}

/// See [`crate::nostr_protocol::build_offer_payload`]'s own doc.
#[wasm_bindgen(js_name = buildOfferPayload)]
pub fn build_offer_payload(sdp: &str, call_id: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::nostr_protocol::build_offer_payload(sdp, call_id))).ok().flatten()
}

/// See [`crate::nostr_protocol::build_answer_payload`]'s own doc.
#[wasm_bindgen(js_name = buildAnswerPayload)]
pub fn build_answer_payload(sdp: &str, call_id: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::nostr_protocol::build_answer_payload(sdp, call_id))).ok().flatten()
}

/// See [`crate::nostr_protocol::build_ice_payload`]'s own doc. `sdp_mid`:
/// `Option<String>` crosses directly (`null`/`undefined` from JS maps
/// straight to `None`), no JNI-style empty-string sentinel needed here.
#[wasm_bindgen(js_name = buildIcePayload)]
pub fn build_ice_payload(sdp_mid: Option<String>, sdp_m_line_index: i32, candidate: &str, call_id: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::nostr_protocol::build_ice_payload(sdp_mid.as_deref(), sdp_m_line_index, candidate, call_id)
    }))
    .ok()
    .flatten()
}

/// See [`crate::nostr_protocol::parse_signal_payload`]'s own doc. Returns
/// a JSON-encoded [`crate::nostr_protocol::SignalMessage`] (`{"type":...,
/// ...}`, same tag/field names as the wire format itself), or `undefined`
/// if the payload doesn't parse as a recognized message or on a panic —
/// same fail-closed posture as `unwrapWrappedEvent`.
#[wasm_bindgen(js_name = parseSignalPayload)]
pub fn parse_signal_payload(payload_json: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let message = crate::nostr_protocol::parse_signal_payload(payload_json)?;
        serde_json::to_string(&message).ok()
    }))
    .ok()
    .flatten()
}

/// See [`crate::ProtocolConstants`]'s own doc. Call once at startup — these
/// never change at runtime — and use the result instead of a hand-copied
/// literal for `SIGNAL_KIND`/`WRAP_KIND`/the passphrase-pairing live-window
/// duration/name-length cap/SDP-and-ICE-candidate length caps. Falls back
/// to the same values re-encoded directly on a panic (this function can't
/// meaningfully fail otherwise — every field is a compile-time constant —
/// but every export in this file gets the same protection on principle).
#[wasm_bindgen(js_name = protocolConstants)]
pub fn protocol_constants() -> String {
    // Static fallback, not a re-invocation of the function that just
    // panicked — re-calling it here, outside catch_unwind, would let a
    // second panic escape uncaught, defeating the point.
    catch_unwind(std::panic::AssertUnwindSafe(|| serde_json::to_string(&crate::protocol_constants()).ok()))
        .ok()
        .flatten()
        .unwrap_or_else(|| "{\"signal_kind\":20331,\"wrap_kind\":20336,\"max_name_length\":100,\"pake_live_window_ms\":120000,\"max_sdp_length\":65536,\"max_ice_candidate_length\":4096}".to_string())
}
