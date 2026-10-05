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
pub fn start_attempt(own_name: &str, passphrase: &str) -> Result<String, JsValue> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = crate::start_attempt(own_name, passphrase);
        serde_json::to_string(&result).map_err(|e| JsValue::from_str(&e.to_string()))
    }))
    .unwrap_or_else(|_| Err(JsValue::from_str("call-core panicked in startAttempt")))
}

/// See [`crate::accept_attempt`]'s own doc. Returns a JSON array of effects (empty if there is nothing to accept).
#[wasm_bindgen(js_name = acceptAttempt)]
pub fn accept_attempt(pairing_id: &str, candidate_pubkey: &str) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        serde_json::to_string(&crate::accept_attempt(pairing_id, candidate_pubkey)).unwrap_or_else(|_| empty_effects_json())
    }))
    .unwrap_or_else(|_| empty_effects_json())
}

/// See [`crate::pending_attempt_ids`]'s own doc. Returns a JSON array of ids.
#[wasm_bindgen(js_name = pendingAttemptIds)]
pub fn pending_attempt_ids() -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| serde_json::to_string(&crate::pending_attempt_ids()).unwrap_or_else(|_| "[]".to_string())))
        .unwrap_or_else(|_| "[]".to_string())
}

/// See [`crate::cancel_attempt`]'s own doc. Returns a JSON array of effects (the signed cancel for the peer, if
/// the exchange had matched).
#[wasm_bindgen(js_name = cancelAttempt)]
pub fn cancel_attempt(pairing_id: &str) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| serde_json::to_string(&crate::cancel_attempt(pairing_id)).unwrap_or_else(|_| empty_effects_json())))
        .unwrap_or_else(|_| empty_effects_json())
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
pub fn request_call(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, peer_online: bool, now_ms: f64) -> Result<String, JsValue> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = crate::call_arbitration::request_call(pairing_id, own_pubkey_hex, peer_pubkey_hex, peer_online, now_ms as i64);
        serde_json::to_string(&result).map_err(|e| JsValue::from_str(&e.to_string()))
    }))
    .unwrap_or_else(|_| Err(JsValue::from_str("call-core panicked in requestCall")))
}

/// See [`crate::call_arbitration::accept_incoming_call`]'s own doc. Returns
/// a JSON-encoded, tagged [`crate::call_arbitration::AcceptOutcome`] (`kind:
/// "ApplyOffer" | "CreateOffer"`), or `undefined` (not an empty string) if
/// there's nothing pending, matching [`build_bootstrap_payload`]'s own
/// `Option` convention — also the fallback on a panic, same sentinel either
/// way, matching `nativeAcceptIncomingCall`'s own convention.
#[wasm_bindgen(js_name = acceptIncomingCall)]
pub fn accept_incoming_call(now_ms: f64) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = crate::call_arbitration::accept_incoming_call(now_ms as i64)?;
        serde_json::to_string(&result).ok()
    }))
    .ok()
    .flatten()
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

/// See [`crate::call_arbitration::check_call_timeout`]'s own doc. Falls back
/// to `[]` on a panic — matches `nativeCheckCallTimeout`'s own reasoning: a
/// failure to safely read state here doesn't warrant tearing down a call
/// that (for all this function could tell) may still be fine; the next tick
/// tries again. `now_ms` is `f64` — same `Date.now()` convention as every
/// other timestamp crossing this boundary.
#[wasm_bindgen(js_name = checkCallTimeout)]
pub fn check_call_timeout(now_ms: f64) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let effects = crate::call_arbitration::check_call_timeout(now_ms as i64);
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
pub fn mark_seen(pairing_id: &str, own_pubkey_hex: &str, peer_pubkey_hex: &str, now_ms: f64, peer_busy: Option<bool>, peer_hello: bool) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let update = crate::presence::mark_seen(pairing_id, own_pubkey_hex, peer_pubkey_hex, now_ms as i64, peer_busy, peer_hello);
        serde_json::to_string(&update).unwrap_or_else(|_| empty_presence_result_json())
    }))
    .unwrap_or_else(|_| empty_presence_result_json())
}

/// See [`crate::presence::request_hello`]'s own doc.
#[wasm_bindgen(js_name = requestHello)]
pub fn request_hello() {
    let _ = catch_unwind(crate::presence::request_hello);
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
/// Returns `u32` directly (a plain JS `number`). Falls back to
/// [`crate::presence::HEARTBEAT_INTERVAL_MS`] (the slow, steady-state
/// cadence) on a panic, matching `nativeCurrentHeartbeatIntervalMs`'s own
/// fail-closed choice — a performance detail, not a correctness one.
#[wasm_bindgen(js_name = currentHeartbeatIntervalMs)]
pub fn current_heartbeat_interval_ms() -> u32 {
    catch_unwind(crate::presence::current_heartbeat_interval_ms).unwrap_or(crate::presence::HEARTBEAT_INTERVAL_MS)
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

/// See [`crate::nostr_protocol::build_bootstrap_event`]'s own doc.
#[wasm_bindgen(js_name = buildBootstrapEvent)]
pub fn build_bootstrap_event(own_private_key_hex: &str, rendezvous_tag: &str, target_pubkey_hex: Option<String>, payload_json: &str) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::nostr_protocol::build_bootstrap_event(own_private_key_hex, rendezvous_tag, target_pubkey_hex.as_deref(), payload_json)
    }))
    .ok()
    .flatten()
}

/// See [`crate::signal_router::route_event`]'s own doc. `context_json` is a
/// JSON-encoded [`crate::signal_router::RouteContext`]. Returns a JSON-encoded
/// [`crate::signal_router::RouteResult`]; a panic routes nothing.
#[wasm_bindgen(js_name = routeEvent)]
pub fn route_event(event_json: &str, context_json: &str, now_ms: f64) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = crate::signal_router::route_event_json(event_json, context_json, now_ms as i64);
        serde_json::to_string(&result).unwrap_or_else(|_| crate::signal_router::empty_result_json())
    }))
    .unwrap_or_else(|_| crate::signal_router::empty_result_json())
}

/// See [`crate::call_arbitration::note_media_failure`]'s own doc.
#[wasm_bindgen(js_name = noteMediaFailure)]
pub fn note_media_failure() {
    let _ = catch_unwind(crate::call_arbitration::note_media_failure);
}

/// See [`crate::call_ui::phase`]'s own doc. Returns a JSON-encoded
/// [`crate::call_ui::PhaseView`]; a panic reads as "no call".
#[wasm_bindgen(js_name = callPhase)]
pub fn call_phase(has_active_call: bool, has_outcome: bool, has_ring: bool, accepted_incoming: bool, peer_connected: bool, peer_offline: bool) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        let view = crate::call_ui::phase(&crate::call_ui::PhaseInput { has_active_call, has_outcome, has_ring, accepted_incoming, peer_connected, peer_offline });
        serde_json::to_string(&view).unwrap_or_default()
    }))
    .ok()
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| r#"{"phase":"idle","show_accept":false,"controls_pinned":false,"label_key":null,"note_key":null}"#.to_string())
}

/// See [`crate::call_ui::pairing_phase`]'s own doc. Returns a JSON-encoded
/// [`crate::call_ui::PairingPhaseView`]; "preparing" on a panic.
#[wasm_bindgen(js_name = pairingPhase)]
pub fn pairing_phase(has_attempt: bool, relays_connected: u32, candidate_found: bool, accepted: bool) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| serde_json::to_string(&crate::call_ui::pairing_phase(has_attempt, relays_connected, candidate_found, accepted)).unwrap_or_default()))
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| r#"{"phase":"preparing","label_key":"pairing.phasePreparing"}"#.to_string())
}

/// See [`crate::call_ui::outcome_text`]'s own doc. `reason`/`diagnosis` are
/// `snake_case` names (`diagnosis` empty or absent for none). Returns the
/// `snake_case` name of the [`crate::call_ui::OutcomeText`] case, falling
/// back to `never_connected`.
#[wasm_bindgen(js_name = outcomeText)]
pub fn outcome_text(reason: &str, diagnosis: Option<String>) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::call_ui::outcome_text_from_names(reason, diagnosis.as_deref().unwrap_or(""))
    }))
    .ok()
    .flatten()
    .unwrap_or_else(|| "never_connected".to_string())
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
pub fn build_bye_payload(call_id: &str, media_failed: bool) -> Option<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::nostr_protocol::build_bye_payload(call_id, media_failed))).ok().flatten()
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

// ---------------------------------------------------------------------------
// Signal-publish retry queue — see `crate::signal_retry`'s own doc for the
// full design. `relays`/`now_ms` follow this file's own existing
// conventions: `Vec<String>` crosses directly as a JS array of strings (see
// `currentHeartbeatIntervalMs`), `now_ms` is `f64` (see `markSeen`'s own
// doc).
// ---------------------------------------------------------------------------

/// See [`crate::signal_retry::record_pending_publish`]'s own doc. No return
/// value; a panic is a silent no-op, same reasoning as every other no-return export here.
#[wasm_bindgen(js_name = recordPendingPublish)]
pub fn record_pending_publish(event_id: &str, event_json: &str, payload_json: &str, relays: Vec<String>, now_ms: f64) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::signal_retry::record_pending_publish(event_id, event_json, payload_json, &relays, now_ms as i64)));
}

/// See [`crate::signal_retry::record_publish_result`]'s own doc. No return
/// value; a panic is a silent no-op.
#[wasm_bindgen(js_name = recordPublishResult)]
pub fn record_publish_result(event_id: &str, relay: &str, accepted: bool, reason: &str, now_ms: f64) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::signal_retry::record_publish_result(event_id, relay, accepted, reason, now_ms as i64)));
}

/// See [`crate::signal_retry::available_relays`]'s own doc. Falls back to
/// "no relays" on a panic — every skipped relay is recorded as a miss by the
/// caller and retried, so nothing is lost.
#[wasm_bindgen(js_name = availableRelays)]
pub fn available_relays(candidates: Vec<String>, now_ms: f64) -> Vec<String> {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::signal_retry::available_relays(&candidates, now_ms as i64))).unwrap_or_default()
}

/// See [`crate::relay_status::export_memory`]'s own doc. A JSON object; `"{}"`
/// on a panic.
#[wasm_bindgen(js_name = exportRelayMemory)]
pub fn export_relay_memory(now_ms: f64) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| crate::relay_status::export_memory(now_ms as i64))).unwrap_or_else(|_| "{}".to_string())
}

/// See [`crate::relay_status::import_memory`]'s own doc. No return value;
/// malformed input or a panic is a silent no-op.
#[wasm_bindgen(js_name = importRelayMemory)]
pub fn import_relay_memory(json: &str, now_ms: f64) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::relay_status::import_memory(json, now_ms as i64)));
}

/// See [`crate::relay_status::note_message`]'s own doc.
#[wasm_bindgen(js_name = noteRelayMessage)]
pub fn note_relay_message(relay: &str, now_ms: f64) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::relay_status::note_message(relay, now_ms as i64)));
}

/// See [`crate::relay_status::note_connected`]'s own doc.
#[wasm_bindgen(js_name = noteRelayConnected)]
pub fn note_relay_connected(relay: &str) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::relay_status::note_connected(relay)));
}

/// See [`crate::relay_status::note_connect_error`]'s own doc.
#[wasm_bindgen(js_name = noteRelayConnectError)]
pub fn note_relay_connect_error(relay: &str, reason: &str, at_ms: f64) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::relay_status::note_connect_error(relay, reason, at_ms as i64)));
}

/// See [`crate::relay_status::view`]'s own doc. Returns a JSON array of
/// [`crate::relay_status::RelayView`]; `"[]"` on a panic.
#[wasm_bindgen(js_name = relayView)]
pub fn relay_view(relays: Vec<String>, connected: Vec<String>, now_ms: f64) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| serde_json::to_string(&crate::relay_status::view(&relays, &connected, now_ms as i64)).unwrap_or_default()))
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "[]".to_string())
}

/// See [`crate::relay_status::ago`]'s own doc. `at_ms` is `undefined` for "no
/// time". Returns a JSON-encoded [`crate::relay_status::Ago`].
#[wasm_bindgen(js_name = ago)]
pub fn ago(at_ms: Option<f64>, now_ms: f64) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| serde_json::to_string(&crate::relay_status::ago(at_ms.map(|ms| ms as i64), now_ms as i64)).unwrap_or_default()))
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| r#"{"unit":"never"}"#.to_string())
}

/// See [`crate::signal_retry::due_for_retry`]'s own doc. Returns a
/// JSON-encoded `Vec<`[`crate::signal_retry::PendingRetry`]`>`, falling back
/// to `"[]"` on a panic — fail-closed, nothing to retry this tick.
#[wasm_bindgen(js_name = dueForRetry)]
pub fn due_for_retry(now_ms: f64) -> String {
    catch_unwind(std::panic::AssertUnwindSafe(|| serde_json::to_string(&crate::signal_retry::due_for_retry(now_ms as i64)).ok()))
        .ok()
        .flatten()
        .unwrap_or_else(|| "[]".to_string())
}

// ---------------------------------------------------------------------------
// ICE evidence / call-failure diagnosis — see `crate::ice_evidence`'s own doc.
// Pure bookkeeping: a panic is a silent no-op, and `iceLastDiagnosis` fails
// closed to "can't tell" (`undefined`).
// ---------------------------------------------------------------------------

/// See [`crate::call_arbitration::can_place_call`]'s own doc. `false` (no
/// Call button) on a panic — the fail-closed choice.
#[wasm_bindgen(js_name = canPlaceCall)]
pub fn can_place_call(connected: bool) -> bool {
    catch_unwind(|| crate::call_arbitration::can_place_call(connected)).unwrap_or(false)
}

/// See [`crate::ice_evidence::reset`]'s own doc.
#[wasm_bindgen(js_name = iceReset)]
pub fn ice_reset() {
    let _ = catch_unwind(crate::ice_evidence::reset);
}

/// See [`crate::ice_evidence::note_local_candidate`]'s own doc.
/// `declared_type` is the browser's own `RTCIceCandidate.type` (`null`/
/// `undefined` when absent).
#[wasm_bindgen(js_name = iceNoteLocalCandidate)]
pub fn ice_note_local_candidate(candidate_line: &str, declared_type: Option<String>) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::ice_evidence::note_local_candidate(candidate_line, declared_type.as_deref())));
}

/// See [`crate::ice_evidence::note_remote_candidate`]'s own doc.
#[wasm_bindgen(js_name = iceNoteRemoteCandidate)]
pub fn ice_note_remote_candidate(candidate_line: &str) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::ice_evidence::note_remote_candidate(candidate_line)));
}

/// See [`crate::ice_evidence::note_connected`]'s own doc.
#[wasm_bindgen(js_name = iceNoteConnected)]
pub fn ice_note_connected() {
    let _ = catch_unwind(crate::ice_evidence::note_connected);
}

/// See [`crate::ice_evidence::remember_diagnosis`]'s own doc.
#[wasm_bindgen(js_name = iceRememberDiagnosis)]
pub fn ice_remember_diagnosis(ice_connection_state: &str) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| crate::ice_evidence::remember_diagnosis(ice_connection_state)));
}

/// See [`crate::ice_evidence::last_diagnosis`]'s own doc. Returns the
/// diagnosis's `snake_case` name (`"no_direct_path"`/`"udp_blocked"`), or
/// `undefined` for "can't tell" (including on a panic).
#[wasm_bindgen(js_name = iceLastDiagnosis)]
pub fn ice_last_diagnosis() -> Option<String> {
    catch_unwind(|| {
        let diagnosis = crate::ice_evidence::last_diagnosis()?;
        serde_json::to_value(diagnosis).ok()?.as_str().map(str::to_string)
    })
    .ok()
    .flatten()
}
