//! JNI glue for `CallCoreBridge.kt` (`android-app`). Hand-rolled, not
//! UniFFI — same reasoning as `pake-bridge/src/android.rs`.
//!
//! Unlike `pake-bridge`'s handle-based design (needed there because a
//! `PakeSession` has real per-call lifecycle to track across the JNI
//! boundary), every function here is a plain string-in/string-out call —
//! `call-core` owns all attempt state internally, keyed by `pairing_id`
//! (see `lib.rs`'s own doc), so there's no session object for the Kotlin
//! side to hold a handle to at all. Structured values cross as JSON;
//! `CallCoreBridge.kt` unwraps them into whatever its own effect-executor
//! loop expects.
//!
//! Every exported function wraps its body in [`catch_unwind`] and returns a
//! null/empty-JSON-array sentinel on panic, same fail-closed posture as
//! `pake-bridge`'s bindings — this crate's input ultimately comes from
//! whatever arrived over a public relay, not trusted, well-formed data.
//!
//! **Four small helpers below** (`encode_nullable`/`encode_or_fallback`/
//! `run_catching`/`encode_bool`) factor out the catch-unwind/serialize/
//! match-into-jstring ceremony every export otherwise repeats by hand. Each
//! export below is now just: extract args from `f`'s parameter, call the
//! crate function, return the result in the shape the chosen helper
//! expects. Two functions don't fit any of the four (`nativeSanitizeName`,
//! whose fail-*open* string return is genuinely unique here, and
//! `nativeCurrentHeartbeatIntervalMs`, the only `jint`-returning export) —
//! left hand-written since there's no real duplication to remove for a
//! single occurrence.

use jni::objects::{JClass, JString};
use jni::sys::{jlong, jstring};
use jni::JNIEnv;
use std::panic::catch_unwind;

fn empty_effects_json() -> String {
    "[]".to_string()
}

fn get_string(env: &mut JNIEnv, s: &JString) -> Option<String> {
    env.get_string(s).ok().map(|s| s.into())
}

/// The `null` sentinel every nullable-result function in this file returns
/// on failure (malformed input or a panic) — a plain `std::ptr::null_mut()`
/// call would do exactly the same thing, but this name self-documents *why*
/// at each call site.
fn null_jstring() -> jstring {
    std::ptr::null_mut()
}

/// Runs `f` inside [`catch_unwind`], turning its `Option<String>` result
/// into a nullable `jstring` — `null` on `None` or a panic. Each call
/// site's own doc explains what specifically makes it `None` (malformed
/// input, no live attempt/pending offer, or a panic) — this helper doesn't
/// distinguish between them, matching every one of those functions'
/// existing fail-closed posture exactly.
fn encode_nullable(env: &mut JNIEnv, f: impl FnOnce(&mut JNIEnv) -> Option<String>) -> jstring {
    match catch_unwind(std::panic::AssertUnwindSafe(|| f(env))).ok().flatten() {
        Some(s) => match env.new_string(s) {
            Ok(s) => s.into_raw(),
            Err(_) => null_jstring(),
        },
        None => null_jstring(),
    }
}

/// Runs `f` inside [`catch_unwind`], JSON-serializing its `Option<T>`
/// result and returning it as a non-nullable `jstring` — `fallback()`'s own
/// JSON on `None` (malformed input) or a panic, never a JNI `null`, so the
/// Kotlin side never needs a separate null-check path for these exports.
fn encode_or_fallback<T: serde::Serialize>(env: &mut JNIEnv, fallback: impl Fn() -> String, f: impl FnOnce(&mut JNIEnv) -> Option<T>) -> jstring {
    let json = catch_unwind(std::panic::AssertUnwindSafe(|| f(env)))
        .ok()
        .flatten()
        .and_then(|v| serde_json::to_string(&v).ok())
        .unwrap_or_else(fallback);
    match env.new_string(json) {
        Ok(s) => s.into_raw(),
        Err(_) => null_jstring(),
    }
}

/// Runs `f` inside [`catch_unwind`], discarding the result — a panic is
/// swallowed the same way every no-return-value export in this file
/// already treats one: pure bookkeeping, nothing for the caller to react to
/// either way.
fn run_catching(env: &mut JNIEnv, f: impl FnOnce(&mut JNIEnv)) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| f(env)));
}

/// Runs `f` inside [`catch_unwind`], turning its `Option<bool>` result into
/// a `jboolean` — `default` on `None` (malformed input) or a panic. Each
/// call site's own doc explains why that particular default is the right
/// fail-closed (or, for `nativeMarkSeenOrIsDuplicate`, deliberately
/// fail-*open*) choice for that specific function.
fn encode_bool(env: &mut JNIEnv, default: bool, f: impl FnOnce(&mut JNIEnv) -> Option<bool>) -> jni::sys::jboolean {
    if catch_unwind(std::panic::AssertUnwindSafe(|| f(env))).ok().flatten().unwrap_or(default) {
        1
    } else {
        0
    }
}

/// Starts a new pairing attempt — see [`crate::start_attempt`]'s own doc.
/// Returns a JSON-encoded [`crate::StartResult`], or `null` on failure
/// (malformed input, or a panic).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeStartAttempt<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    own_pubkey_hex: JString<'local>,
    own_name: JString<'local>,
    passphrase: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let own_pubkey_hex = get_string(env, &own_pubkey_hex)?;
        let own_name = get_string(env, &own_name)?;
        let passphrase = get_string(env, &passphrase)?;
        let start_result = crate::start_attempt(&pairing_id, &own_pubkey_hex, &own_name, &passphrase);
        serde_json::to_string(&start_result).ok()
    })
}

/// See [`crate::cancel_attempt`]'s own doc.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeCancelAttempt<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
) {
    run_catching(&mut env, |env| {
        if let Some(pairing_id) = get_string(env, &pairing_id) {
            crate::cancel_attempt(&pairing_id);
        }
    });
}

/// See [`crate::build_bootstrap_payload`]'s own doc. Returns `null` if
/// there's no live attempt (or on a panic) — the same sentinel either way,
/// matching that function's own `Option` return.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildBootstrapPayload<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        crate::build_bootstrap_payload(&pairing_id)
    })
}

/// See [`crate::handle_bootstrap_message`]'s own doc. Returns a
/// JSON-encoded array of [`crate::Effect`] — always a valid JSON array
/// (possibly empty, `[]`), never `null`, including on a panic, so the
/// Kotlin side never needs a separate null-check path here the way the
/// other functions in this file do.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeHandleBootstrapMessage<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    sender_pubkey: JString<'local>,
    type_: JString<'local>,
    payload_json: JString<'local>,
) -> jstring {
    encode_or_fallback(&mut env, empty_effects_json, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let sender_pubkey = get_string(env, &sender_pubkey)?;
        let type_ = get_string(env, &type_)?;
        let payload_json = get_string(env, &payload_json)?;
        Some(crate::handle_bootstrap_message(&pairing_id, &sender_pubkey, &type_, &payload_json))
    })
}

/// See [`crate::handle_timeout`]'s own doc. `generation` must be exactly
/// the value returned in `nativeStartAttempt`'s JSON result — same
/// always-a-JSON-array contract as `nativeHandleBootstrapMessage`.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeHandleTimeout<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    generation: jlong,
) -> jstring {
    encode_or_fallback(&mut env, empty_effects_json, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        Some(crate::handle_timeout(&pairing_id, generation as u64))
    })
}

/// See [`crate::sanitize_name`]'s own doc — exposed standalone too, so
/// `NameSanitize.kt`'s other call sites (own device name entry/rename, not
/// just the pairing flow) can be replaced by this single implementation
/// instead of keeping a second hand-copy around now that this crate needs
/// the exact same operation internally anyway. Returns the original string
/// unchanged on a panic (fails open, deliberately, unlike every other
/// function in this file) — this is display sanitization, not a security
/// boundary on its own; returning unsanitized text on an internal panic is
/// safer for the human reading it than silently returning `null`/empty and
/// losing their typed name entirely.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeSanitizeName<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
) -> jstring {
    let Some(name) = get_string(&mut env, &name) else {
        return null_jstring();
    };
    let sanitized = catch_unwind(std::panic::AssertUnwindSafe(|| crate::sanitize_name(&name))).unwrap_or(name);
    match env.new_string(sanitized) {
        Ok(s) => s.into_raw(),
        Err(_) => null_jstring(),
    }
}

// ---------------------------------------------------------------------------
// Call arbitration — see `crate::call_arbitration`'s own doc for the full
// design. Same conventions as the pairing-bootstrap bindings above:
// `catch_unwind` at every boundary, JSON for structured values, a plain
// string-in/string-out shape throughout. `sdp_mid` (nullable on the Kotlin
// side, `String?`) crosses as a possibly-*empty* (never null) JString —
// Kotlin passes `""` for `null`, decoded back to `None` here — since a real
// ICE `sdpMid` is never itself an empty string, this is a safe, simple
// sentinel that avoids JNI's own null-JString handling entirely.
// ---------------------------------------------------------------------------

/// See [`crate::call_arbitration::request_call`]'s own doc. Returns a
/// JSON-encoded [`crate::call_arbitration::RequestCallResult`], or `null`
/// on failure (malformed input, or a panic).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeRequestCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    own_pubkey_hex: JString<'local>,
    peer_pubkey_hex: JString<'local>,
    peer_online: jni::sys::jboolean,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let own_pubkey_hex = get_string(env, &own_pubkey_hex)?;
        let peer_pubkey_hex = get_string(env, &peer_pubkey_hex)?;
        let result = crate::call_arbitration::request_call(&pairing_id, &own_pubkey_hex, &peer_pubkey_hex, peer_online != 0);
        serde_json::to_string(&result).ok()
    })
}

/// See [`crate::call_arbitration::handle_should_offer`]'s own doc — the
/// pubkey tie-break lives inside that function itself, so the shell just
/// forwards both pubkeys through unconditionally like every other message
/// type. `auto_answer` is looked up by the caller (its own contacts list)
/// the same way `nativeHandleOffer`'s already is.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeHandleShouldOffer<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    call_id: JString<'local>,
    own_pubkey_hex: JString<'local>,
    peer_pubkey_hex: JString<'local>,
    auto_answer: jni::sys::jboolean,
) -> jstring {
    encode_or_fallback(&mut env, empty_effects_json, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let call_id = get_string(env, &call_id)?;
        let own_pubkey_hex = get_string(env, &own_pubkey_hex)?;
        let peer_pubkey_hex = get_string(env, &peer_pubkey_hex)?;
        Some(crate::call_arbitration::handle_should_offer(&pairing_id, &call_id, &own_pubkey_hex, &peer_pubkey_hex, auto_answer != 0))
    })
}

/// See [`crate::call_arbitration::handle_offer`]'s own doc — no longer
/// takes a `has_active_peer_connection` flag (dropped from the Rust
/// signature once found to be provably redundant with `offer_applied`).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeHandleOffer<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    call_id: JString<'local>,
    sdp: JString<'local>,
    auto_answer: jni::sys::jboolean,
) -> jstring {
    encode_or_fallback(&mut env, empty_effects_json, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let call_id = get_string(env, &call_id)?;
        let sdp = get_string(env, &sdp)?;
        Some(crate::call_arbitration::handle_offer(&pairing_id, &call_id, &sdp, auto_answer != 0))
    })
}

/// See [`crate::call_arbitration::should_apply_answer`]'s own doc. Returns
/// `false` (not just "unknown") on a panic, matching this function's own
/// fail-closed posture elsewhere — an unapplied answer just means the call
/// silently sits at `have-local-offer` a little longer, not a security or
/// correctness problem the way applying a bogus one would be.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeShouldApplyAnswer<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    call_id: JString<'local>,
) -> jni::sys::jboolean {
    encode_bool(&mut env, false, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let call_id = get_string(env, &call_id)?;
        Some(crate::call_arbitration::should_apply_answer(&pairing_id, &call_id))
    })
}

/// See [`crate::call_arbitration::handle_remote_ice`]'s own doc. `sdp_mid`:
/// see this section's own top-of-file note — an empty string means `null`.
/// Returns a JSON-encoded [`crate::call_arbitration::IceOutcome`], falling
/// back to `Dropped` on a panic (fail-closed: silently discarding one ICE
/// candidate is harmless — trickle ICE sends several — applying/buffering
/// one from a call state that couldn't even be read safely is not worth
/// the risk).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeHandleRemoteIce<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    call_id: JString<'local>,
    sdp_mid: JString<'local>,
    sdp_m_line_index: jni::sys::jint,
    candidate: JString<'local>,
) -> jstring {
    encode_or_fallback(&mut env, || "{\"outcome\":\"Dropped\"}".to_string(), |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let call_id = get_string(env, &call_id)?;
        let sdp_mid = get_string(env, &sdp_mid)?;
        let sdp_mid = if sdp_mid.is_empty() { None } else { Some(sdp_mid.as_str()) };
        let candidate = get_string(env, &candidate)?;
        Some(crate::call_arbitration::handle_remote_ice(&pairing_id, &call_id, sdp_mid, sdp_m_line_index, &candidate))
    })
}

/// See [`crate::call_arbitration::accept_incoming_call`]'s own doc. Returns
/// a JSON-encoded, tagged [`crate::call_arbitration::AcceptOutcome`] (`kind:
/// "ApplyOffer" | "CreateOffer"`), or `null` if there's nothing pending (or
/// on a panic — same sentinel either way, matching
/// `nativeBuildBootstrapPayload`'s own convention above).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeAcceptIncomingCall<'local>(mut env: JNIEnv<'local>, _class: JClass<'local>) -> jstring {
    encode_nullable(&mut env, |_env| {
        let accept_result = crate::call_arbitration::accept_incoming_call()?;
        serde_json::to_string(&accept_result).ok()
    })
}

/// See [`crate::call_arbitration::tick_incoming_call_countdown`]'s own doc.
/// Falls back to `Stale` on a panic — the fail-closed choice here (a
/// countdown that silently stops rather than one that might auto-accept a
/// call it couldn't safely reason about).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeTickIncomingCallCountdown<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    call_id: JString<'local>,
) -> jstring {
    encode_or_fallback(&mut env, || "{\"outcome\":\"Stale\"}".to_string(), |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let call_id = get_string(env, &call_id)?;
        Some(crate::call_arbitration::tick_incoming_call_countdown(&pairing_id, &call_id))
    })
}

/// See [`crate::call_arbitration::hang_up`]'s own doc. Falls back to just
/// `[ClosePeerConnection]` on a panic — even if this module's own state
/// couldn't be safely read/cleared, the shell must still be told to tear
/// down the real `PeerConnection`; losing that would leak a live call.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeHangUp<'local>(mut env: JNIEnv<'local>, _class: JClass<'local>) -> jstring {
    encode_or_fallback(&mut env, || "[{\"kind\":\"ClosePeerConnection\"}]".to_string(), |_env| {
        Some(crate::call_arbitration::hang_up())
    })
}

/// See [`crate::call_arbitration::handle_peer_hangup`]'s own doc.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeHandlePeerHangup<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    call_id: JString<'local>,
) -> jstring {
    encode_or_fallback(&mut env, empty_effects_json, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let call_id = get_string(env, &call_id)?;
        Some(crate::call_arbitration::handle_peer_hangup(&pairing_id, &call_id))
    })
}

/// See [`crate::call_arbitration::handle_peer_busy`]'s own doc.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeHandlePeerBusy<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    call_id: JString<'local>,
) -> jstring {
    encode_or_fallback(&mut env, empty_effects_json, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let call_id = get_string(env, &call_id)?;
        Some(crate::call_arbitration::handle_peer_busy(&pairing_id, &call_id))
    })
}

/// See [`crate::call_arbitration::peer_connection_closed`]'s own doc.
/// **Now returns a JSON-encoded effects array** (was `void`) — that
/// function's own signature changed from `()` to `Vec<CallEffect>` so a
/// genuinely spontaneous WebRTC teardown can carry a `ShowCallOutcome`
/// down to the shell; same always-a-JSON-array contract (empty array on a
/// panic) as the other effect-returning functions in this file.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativePeerConnectionClosed<'local>(mut env: JNIEnv<'local>, _class: JClass<'local>) -> jstring {
    encode_or_fallback(&mut env, empty_effects_json, |_env| Some(crate::call_arbitration::peer_connection_closed()))
}

/// See [`crate::call_arbitration::mark_connected`]'s own doc. No return
/// value — pure bookkeeping, same posture as `nativeCancelAttempt` above.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeMarkConnected<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    call_id: JString<'local>,
) {
    run_catching(&mut env, |env| {
        let (Some(pairing_id), Some(call_id)) = (get_string(env, &pairing_id), get_string(env, &call_id)) else { return };
        crate::call_arbitration::mark_connected(&pairing_id, &call_id);
    });
}

/// See [`crate::call_arbitration::should_end_call_on_media_failure`]'s own
/// doc. Falls back to `true` (end the call) on a panic — the safer
/// direction when this module's own logic couldn't be trusted: a call
/// wrongly ended is recoverable (redial); a media failure wrongly treated
/// as harmless during an actual live call is a worse user experience (a
/// dead call that looks alive).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeShouldEndCallOnMediaFailure<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    has_active_peer_connection: jni::sys::jboolean,
) -> jni::sys::jboolean {
    encode_bool(&mut env, true, |_env| Some(crate::call_arbitration::should_end_call_on_media_failure(has_active_peer_connection != 0)))
}

/// See [`crate::call_arbitration::forget_pairing`]'s own doc. Same
/// always-a-JSON-array contract as the other effect-returning functions
/// above.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeForgetPairing<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
) -> jstring {
    encode_or_fallback(&mut env, empty_effects_json, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        Some(crate::call_arbitration::forget_pairing(&pairing_id))
    })
}

// ---------------------------------------------------------------------------
// Presence — see `crate::presence`'s own doc for the full design. Same
// conventions as above; `Vec<String>` parameters (`pending_pairing_ids`)
// cross as a JSON array string, decoded with serde_json — consistent with
// this file's "structured values cross as JSON" convention rather than
// hand-marshaling a JNI string array.
// ---------------------------------------------------------------------------

fn empty_presence_result_json() -> String {
    "{\"presence_effects\":[],\"call_effects\":[]}".to_string()
}

/// See [`crate::presence::mark_seen`]'s own doc. Returns a JSON-encoded
/// [`crate::presence::PresenceUpdateResult`], falling back to an empty one
/// on a panic (fail-closed: no UI transition, no call effect — safer than
/// guessing). `peer_busy`: JNI has no clean nullable primitive, so this
/// crosses as a tri-state `jint` — `-1` means "no info" (`None`, every
/// non-heartbeat message type), `0`/`1` mean `Some(false)`/`Some(true)` —
/// same sentinel-value convention this file already uses elsewhere for a
/// nullable value crossing the JNI boundary (e.g. `sdpMid`'s empty-string-
/// means-`None`), rather than a second parameter pair.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeMarkSeen<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    own_pubkey_hex: JString<'local>,
    peer_pubkey_hex: JString<'local>,
    now_ms: jlong,
    peer_busy: jni::sys::jint,
) -> jstring {
    encode_or_fallback(&mut env, empty_presence_result_json, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let own_pubkey_hex = get_string(env, &own_pubkey_hex)?;
        let peer_pubkey_hex = get_string(env, &peer_pubkey_hex)?;
        let peer_busy = match peer_busy {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        Some(crate::presence::mark_seen(&pairing_id, &own_pubkey_hex, &peer_pubkey_hex, now_ms, peer_busy))
    })
}

/// See [`crate::presence::handle_peer_busy_reply`]'s own doc. Takes the same
/// `own_pubkey_hex`/`peer_pubkey_hex` tie-break inputs `nativeMarkSeen`
/// does — this function now performs its own online transition rather than
/// relying on the shell having already called `markSeen` first, so it needs
/// the same two pieces of context that transition requires.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeHandlePeerBusyReply<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
    own_pubkey_hex: JString<'local>,
    peer_pubkey_hex: JString<'local>,
    call_id: JString<'local>,
) -> jstring {
    encode_or_fallback(&mut env, empty_presence_result_json, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        let own_pubkey_hex = get_string(env, &own_pubkey_hex)?;
        let peer_pubkey_hex = get_string(env, &peer_pubkey_hex)?;
        let call_id = get_string(env, &call_id)?;
        Some(crate::presence::handle_peer_busy_reply(&pairing_id, &own_pubkey_hex, &peer_pubkey_hex, &call_id))
    })
}

/// See [`crate::presence::is_peer_busy`]'s own doc. Returns `false` on a
/// panic, matching [`nativeIsOnline`]'s own "never seen" sentinel.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeIsPeerBusy<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
) -> jni::sys::jboolean {
    encode_bool(&mut env, false, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        Some(crate::presence::is_peer_busy(&pairing_id))
    })
}

/// See [`crate::call_arbitration::is_call_active`]'s own doc. Returns
/// `true` (fail toward *not* claiming to be free) on a panic — this feeds
/// the shell's own outgoing heartbeat's busy field, and wrongly
/// broadcasting "not busy" while this module's own logic couldn't be
/// trusted is the worse direction to fail in.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeIsCallActive<'local>(mut env: JNIEnv<'local>, _class: JClass<'local>) -> jni::sys::jboolean {
    encode_bool(&mut env, true, |_env| Some(crate::call_arbitration::is_call_active()))
}

/// See [`crate::presence::handle_leaving_message`]'s own doc.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeHandleLeavingMessage<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
) -> jstring {
    encode_or_fallback(&mut env, empty_presence_result_json, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        Some(crate::presence::handle_leaving_message(&pairing_id))
    })
}

/// See [`crate::presence::check_online_timeouts`]'s own doc.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeCheckOnlineTimeouts<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    now_ms: jlong,
) -> jstring {
    encode_or_fallback(&mut env, empty_presence_result_json, |_env| Some(crate::presence::check_online_timeouts(now_ms)))
}

/// See [`crate::presence::is_online`]'s own doc. Returns `false` on a
/// panic, matching that function's own "never seen" sentinel.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeIsOnline<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
) -> jni::sys::jboolean {
    encode_bool(&mut env, false, |env| {
        let pairing_id = get_string(env, &pairing_id)?;
        Some(crate::presence::is_online(&pairing_id))
    })
}

/// See [`crate::presence::current_heartbeat_interval_ms`]'s own doc.
/// `pending_pairing_ids_json` is a JSON array of pairing-id strings.
/// Returns [`crate::presence::HEARTBEAT_INTERVAL_MS`] (the slow, steady-
/// state cadence) on a panic or malformed input — the fail-closed choice,
/// matching [`crate::call_arbitration::any_call_wanted`]'s own fail-closed
/// reasoning: a performance detail, not a correctness one.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeCurrentHeartbeatIntervalMs<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pending_pairing_ids_json: JString<'local>,
    now_ms: jlong,
) -> jni::sys::jint {
    let result = catch_unwind(std::panic::AssertUnwindSafe(|| {
        let json = get_string(&mut env, &pending_pairing_ids_json)?;
        let pending: Vec<String> = serde_json::from_str(&json).ok()?;
        Some(crate::presence::current_heartbeat_interval_ms(&pending, now_ms))
    }));
    result.ok().flatten().unwrap_or(crate::presence::HEARTBEAT_INTERVAL_MS) as jni::sys::jint
}

/// See [`crate::presence::prune_stale_pending`]'s own doc.
/// `current_pending_ids_json` is a JSON array of pairing-id strings. No
/// return value; malformed input or a panic is a silent no-op (the map just
/// doesn't get pruned this tick — it'll be pruned next time, same as any
/// other missed tick).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativePruneStalePending<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    current_pending_ids_json: JString<'local>,
) {
    run_catching(&mut env, |env| {
        let Some(json) = get_string(env, &current_pending_ids_json) else { return };
        let Ok(current) = serde_json::from_str::<Vec<String>>(&json) else { return };
        crate::presence::prune_stale_pending(&current);
    });
}

/// See [`crate::presence::remove_pairing`]'s own doc. No return value; a
/// panic here is swallowed the same way [`nativePeerConnectionClosed`]
/// swallows one.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativePresenceRemovePairing<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    pairing_id: JString<'local>,
) {
    run_catching(&mut env, |env| {
        if let Some(pairing_id) = get_string(env, &pairing_id) {
            crate::presence::remove_pairing(&pairing_id);
        }
    });
}

// ---------------------------------------------------------------------------
// Nostr protocol — see `crate::nostr_protocol`'s own doc for the full
// design. Same conventions as everywhere else in this file: `catch_unwind`
// at every boundary, JSON for structured values and `Vec<String>`
// parameters (no native JNI string-array support, same reasoning as the
// presence bindings above). `target_pubkey_hex` (nullable on the Kotlin
// side, `String?`) crosses as a possibly-*empty* JString — Kotlin passes
// `""` for `null`, decoded back to `None` here — since a real pubkey hex
// is never itself an empty string, this is a safe, simple sentinel that
// avoids JNI's own null-JString handling, the same pattern already used
// for ICE's `sdpMid` in the call-arbitration bindings above.
// ---------------------------------------------------------------------------

/// See [`crate::nostr_protocol::build_wrapped_event`]'s own doc. Returns
/// the signed outer event as JSON, or `null` on any failure (malformed
/// input, or a panic).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildWrappedEvent<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    own_private_key_hex: JString<'local>,
    target_pubkey_hex: JString<'local>,
    payload_json: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let own_private_key_hex = get_string(env, &own_private_key_hex)?;
        let target_pubkey_hex = get_string(env, &target_pubkey_hex)?;
        let payload_json = get_string(env, &payload_json)?;
        crate::nostr_protocol::build_wrapped_event(&own_private_key_hex, &target_pubkey_hex, &payload_json)
    })
}

/// See [`crate::nostr_protocol::unwrap_wrapped_event_for_any`]'s own doc.
/// `candidates_json` is a JSON array of
/// [`crate::nostr_protocol::WrapEventCandidate`] (this device's confirmed
/// peers — `pairing_id`/`own_private_key_hex`/`peer_public_key` each).
/// Returns a JSON-encoded [`crate::nostr_protocol::RoutedSignalPayload`],
/// or `null` if the event doesn't route to any of `candidates`, fails to
/// decrypt/verify once routed, is malformed, or on a panic — same
/// fail-closed posture as every other "a confirmed peer's message this
/// device can't safely interpret" case in this file.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeUnwrapWrappedEventForAny<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    wrap_event_json: JString<'local>,
    candidates_json: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let wrap_event_json = get_string(env, &wrap_event_json)?;
        let candidates_json = get_string(env, &candidates_json)?;
        let candidates: Vec<crate::nostr_protocol::WrapEventCandidate> = serde_json::from_str(&candidates_json).ok()?;
        let routed = crate::nostr_protocol::unwrap_wrapped_event_for_any(&wrap_event_json, &candidates)?;
        serde_json::to_string(&routed).ok()
    })
}

/// See [`crate::nostr_protocol::build_bootstrap_event`]'s own doc.
/// `target_pubkey_hex`: see this section's own top-of-file note — an
/// empty string means `null` (no candidate pubkey known yet).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildBootstrapEvent<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    own_private_key_hex: JString<'local>,
    rendezvous_tag: JString<'local>,
    target_pubkey_hex: JString<'local>,
    payload_json: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let own_private_key_hex = get_string(env, &own_private_key_hex)?;
        let rendezvous_tag = get_string(env, &rendezvous_tag)?;
        let target_pubkey_hex = get_string(env, &target_pubkey_hex)?;
        let target = if target_pubkey_hex.is_empty() { None } else { Some(target_pubkey_hex.as_str()) };
        let payload_json = get_string(env, &payload_json)?;
        crate::nostr_protocol::build_bootstrap_event(&own_private_key_hex, &rendezvous_tag, target, &payload_json)
    })
}

/// See [`crate::nostr_protocol::verify_bootstrap_event`]'s own doc.
/// Returns a JSON-encoded [`crate::nostr_protocol::VerifiedBootstrapEvent`],
/// or `null` if verification fails (or on a panic — same sentinel either
/// way, matching that function's own `Option` return).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeVerifyBootstrapEvent<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    event_json: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let event_json = get_string(env, &event_json)?;
        let verified = crate::nostr_protocol::verify_bootstrap_event(&event_json)?;
        serde_json::to_string(&verified).ok()
    })
}

/// See [`crate::nostr_protocol::mark_seen_or_is_duplicate`]'s own doc.
/// Falls back to `true` (treat as new, let it through) on a panic —
/// deliberately fail-*open* here, unlike most of this file: a redelivered
/// message wrongly let through a second time is already guarded downstream
/// (`call_arbitration`'s own busy/redelivery checks, invariant #7 —
/// re-processing an already-seen offer/call is idempotent by design), but
/// a dedup check that panicked and then defaulted to "always a duplicate"
/// would silently and permanently block every future message on this
/// connection — a far worse outcome than one possible double-process.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeMarkSeenOrIsDuplicate<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    event_id: JString<'local>,
) -> jni::sys::jboolean {
    encode_bool(&mut env, true, |env| {
        let event_id = get_string(env, &event_id)?;
        Some(crate::nostr_protocol::mark_seen_or_is_duplicate(&event_id))
    })
}

/// See [`crate::nostr_protocol::build_relay_filters`]'s own doc.
/// `confirmed_own_pubkeys_json`/`pending_rendezvous_tags_json` are each a
/// JSON array of strings. Returns a JSON-encoded
/// [`crate::nostr_protocol::RelayFilters`] — falls back to
/// `{"wrap_filter":null,"bootstrap_filter":null}` on malformed input or a
/// panic, the same "nothing to filter for" no-op both platforms' own
/// "omit if empty" behavior already treats as unremarkable.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildRelayFilters<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    confirmed_own_pubkeys_json: JString<'local>,
    pending_rendezvous_tags_json: JString<'local>,
) -> jstring {
    encode_or_fallback(&mut env, || "{\"wrap_filter\":null,\"bootstrap_filter\":null}".to_string(), |env| {
        let confirmed_json = get_string(env, &confirmed_own_pubkeys_json)?;
        let pending_json = get_string(env, &pending_rendezvous_tags_json)?;
        let confirmed: Vec<String> = serde_json::from_str(&confirmed_json).ok()?;
        let pending: Vec<String> = serde_json::from_str(&pending_json).ok()?;
        Some(crate::nostr_protocol::build_relay_filters(&confirmed, &pending))
    })
}

/// See [`crate::nostr_protocol::build_heartbeat_payload`]'s own doc.
/// Returns the wire JSON, or `null` on a panic (this one never fails on
/// input alone — every argument is already a plain owned value).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildHeartbeatPayload<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
    busy: jni::sys::jboolean,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let name = get_string(env, &name)?;
        crate::nostr_protocol::build_heartbeat_payload(&name, busy != 0)
    })
}

/// See [`crate::nostr_protocol::build_leaving_payload`]'s own doc.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildLeavingPayload<'local>(mut env: JNIEnv<'local>, _class: JClass<'local>) -> jstring {
    encode_nullable(&mut env, |_env| crate::nostr_protocol::build_leaving_payload())
}

/// See [`crate::nostr_protocol::build_bye_payload`]'s own doc.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildByePayload<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    call_id: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let call_id = get_string(env, &call_id)?;
        crate::nostr_protocol::build_bye_payload(&call_id)
    })
}

/// See [`crate::nostr_protocol::build_busy_payload`]'s own doc.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildBusyPayload<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    call_id: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let call_id = get_string(env, &call_id)?;
        crate::nostr_protocol::build_busy_payload(&call_id)
    })
}

/// See [`crate::nostr_protocol::build_call_payload`]'s own doc.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildCallPayload<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    call_id: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let call_id = get_string(env, &call_id)?;
        crate::nostr_protocol::build_call_payload(&call_id)
    })
}

/// See [`crate::nostr_protocol::build_offer_payload`]'s own doc.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildOfferPayload<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    sdp: JString<'local>,
    call_id: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let sdp = get_string(env, &sdp)?;
        let call_id = get_string(env, &call_id)?;
        crate::nostr_protocol::build_offer_payload(&sdp, &call_id)
    })
}

/// See [`crate::nostr_protocol::build_answer_payload`]'s own doc.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildAnswerPayload<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    sdp: JString<'local>,
    call_id: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let sdp = get_string(env, &sdp)?;
        let call_id = get_string(env, &call_id)?;
        crate::nostr_protocol::build_answer_payload(&sdp, &call_id)
    })
}

/// See [`crate::nostr_protocol::build_ice_payload`]'s own doc. `sdp_mid`:
/// see this section's own top-of-file note — an empty string means `None`,
/// the same convention `nativeHandleRemoteIce` already uses.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeBuildIcePayload<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    sdp_mid: JString<'local>,
    sdp_m_line_index: jni::sys::jint,
    candidate: JString<'local>,
    call_id: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let sdp_mid = get_string(env, &sdp_mid)?;
        let sdp_mid = if sdp_mid.is_empty() { None } else { Some(sdp_mid.as_str()) };
        let candidate = get_string(env, &candidate)?;
        let call_id = get_string(env, &call_id)?;
        crate::nostr_protocol::build_ice_payload(sdp_mid, sdp_m_line_index, &candidate, &call_id)
    })
}

/// See [`crate::nostr_protocol::parse_signal_payload`]'s own doc. Returns
/// a JSON-encoded [`crate::nostr_protocol::SignalMessage`] (`{"type":...,
/// ...}`, same tag/field names as the wire format itself — this is a
/// decoded value, not a fresh envelope), or `null` if the payload doesn't
/// parse as a recognized message (or on a panic — same fail-closed
/// posture as `nativeUnwrapWrappedEvent`: a message this device can't
/// safely interpret is dropped silently, not guessed at).
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeParseSignalPayload<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    payload_json: JString<'local>,
) -> jstring {
    encode_nullable(&mut env, |env| {
        let payload_json = get_string(env, &payload_json)?;
        let message = crate::nostr_protocol::parse_signal_payload(&payload_json)?;
        serde_json::to_string(&message).ok()
    })
}

/// See [`crate::ProtocolConstants`]'s own doc. Call once at startup — these
/// never change at runtime — and use the result instead of a hand-copied
/// literal for `SIGNAL_KIND`/`WRAP_KIND`/the passphrase-pairing live-window
/// duration/name-length cap/SDP-and-ICE-candidate length caps.
#[no_mangle]
pub extern "system" fn Java_dev_porchlight_app_CallCoreBridge_nativeProtocolConstants<'local>(mut env: JNIEnv<'local>, _class: JClass<'local>) -> jstring {
    // Static fallback, not a re-invocation of protocol_constants() — that
    // would run outside catch_unwind's safety net, so a genuine panic in it
    // would escape uncaught on retry instead of being contained.
    encode_or_fallback(
        &mut env,
        || "{\"signal_kind\":20331,\"wrap_kind\":20336,\"max_name_length\":100,\"pake_live_window_ms\":120000,\"max_sdp_length\":65536,\"max_ice_candidate_length\":4096}".to_string(),
        |_env| Some(crate::protocol_constants()),
    )
}
