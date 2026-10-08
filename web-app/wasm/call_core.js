/* @ts-self-types="./call_core.d.ts" */

/**
 * See [`crate::accept_attempt`]'s own doc. Returns a JSON array of effects (empty if there is nothing to accept).
 * @param {string} pairing_id
 * @param {string} candidate_pubkey
 * @returns {string}
 */
export function acceptAttempt(pairing_id, candidate_pubkey) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(pairing_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(candidate_pubkey, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.acceptAttempt(ptr0, len0, ptr1, len1);
        deferred3_0 = ret[0];
        deferred3_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * See [`crate::call_arbitration::accept_incoming_call`]'s own doc. Returns
 * a JSON-encoded, tagged [`crate::call_arbitration::AcceptOutcome`] (`kind:
 * "ApplyOffer" | "CreateOffer"`), or `undefined` (not an empty string) if
 * there's nothing pending, matching [`build_bootstrap_payload`]'s own
 * `Option` convention — also the fallback on a panic, same sentinel either
 * way, matching `nativeAcceptIncomingCall`'s own convention.
 * @param {number} now_ms
 * @returns {string | undefined}
 */
export function acceptIncomingCall(now_ms) {
    const ret = wasm.acceptIncomingCall(now_ms);
    let v1;
    if (ret[0] !== 0) {
        v1 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v1;
}

/**
 * See [`crate::relay_status::ago`]'s own doc. `at_ms` is `undefined` for "no
 * time". Returns a JSON-encoded [`crate::relay_status::Ago`].
 * @param {number | null | undefined} at_ms
 * @param {number} now_ms
 * @returns {string}
 */
export function ago(at_ms, now_ms) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.ago(!isLikeNone(at_ms), isLikeNone(at_ms) ? 0 : at_ms, now_ms);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::signal_retry::available_relays`]'s own doc. Falls back to
 * "no relays" on a panic — every skipped relay is recorded as a miss by the
 * caller and retried, so nothing is lost.
 * @param {string[]} candidates
 * @param {number} now_ms
 * @returns {string[]}
 */
export function availableRelays(candidates, now_ms) {
    const ptr0 = passArrayJsValueToWasm0(candidates, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.availableRelays(ptr0, len0, now_ms);
    var v2 = getArrayJsValueFromWasm0(ret[0], ret[1]);
    wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
    return v2;
}

/**
 * See [`crate::nostr_protocol::build_answer_payload`]'s own doc.
 * @param {string} sdp
 * @param {string} call_id
 * @returns {string | undefined}
 */
export function buildAnswerPayload(sdp, call_id) {
    const ptr0 = passStringToWasm0(sdp, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(call_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.buildAnswerPayload(ptr0, len0, ptr1, len1);
    let v3;
    if (ret[0] !== 0) {
        v3 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v3;
}

/**
 * See [`crate::nostr_protocol::build_bootstrap_event`]'s own doc.
 * @param {string} own_private_key_hex
 * @param {string} rendezvous_tag
 * @param {string | null | undefined} target_pubkey_hex
 * @param {string} payload_json
 * @returns {string | undefined}
 */
export function buildBootstrapEvent(own_private_key_hex, rendezvous_tag, target_pubkey_hex, payload_json) {
    const ptr0 = passStringToWasm0(own_private_key_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(rendezvous_tag, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    var ptr2 = isLikeNone(target_pubkey_hex) ? 0 : passStringToWasm0(target_pubkey_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    var len2 = WASM_VECTOR_LEN;
    const ptr3 = passStringToWasm0(payload_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len3 = WASM_VECTOR_LEN;
    const ret = wasm.buildBootstrapEvent(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
    let v5;
    if (ret[0] !== 0) {
        v5 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v5;
}

/**
 * See [`crate::build_bootstrap_payload`]'s own doc. Returns `undefined`
 * (not an empty string) if there's no live attempt, matching that
 * function's own `Option` return in a way `app.js` can check with a plain
 * falsy test — also the fallback on a panic, same sentinel either way,
 * matching `nativeBuildBootstrapPayload`'s own convention.
 * @param {string} pairing_id
 * @returns {string | undefined}
 */
export function buildBootstrapPayload(pairing_id) {
    const ptr0 = passStringToWasm0(pairing_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.buildBootstrapPayload(ptr0, len0);
    let v2;
    if (ret[0] !== 0) {
        v2 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v2;
}

/**
 * See [`crate::nostr_protocol::build_busy_payload`]'s own doc.
 * @param {string} call_id
 * @returns {string | undefined}
 */
export function buildBusyPayload(call_id) {
    const ptr0 = passStringToWasm0(call_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.buildBusyPayload(ptr0, len0);
    let v2;
    if (ret[0] !== 0) {
        v2 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v2;
}

/**
 * See [`crate::nostr_protocol::build_bye_payload`]'s own doc.
 * @param {string} call_id
 * @param {boolean} media_failed
 * @returns {string | undefined}
 */
export function buildByePayload(call_id, media_failed) {
    const ptr0 = passStringToWasm0(call_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.buildByePayload(ptr0, len0, media_failed);
    let v2;
    if (ret[0] !== 0) {
        v2 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v2;
}

/**
 * See [`crate::nostr_protocol::build_call_payload`]'s own doc.
 * @param {string} call_id
 * @returns {string | undefined}
 */
export function buildCallPayload(call_id) {
    const ptr0 = passStringToWasm0(call_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.buildCallPayload(ptr0, len0);
    let v2;
    if (ret[0] !== 0) {
        v2 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v2;
}

/**
 * See [`crate::nostr_protocol::build_heartbeat_payload`]'s own doc.
 * @param {string} name
 * @param {boolean} busy
 * @returns {string | undefined}
 */
export function buildHeartbeatPayload(name, busy) {
    const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.buildHeartbeatPayload(ptr0, len0, busy);
    let v2;
    if (ret[0] !== 0) {
        v2 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v2;
}

/**
 * See [`crate::nostr_protocol::build_ice_payload`]'s own doc. `sdp_mid`:
 * `Option<String>` crosses directly (`null`/`undefined` from JS maps
 * straight to `None`), no JNI-style empty-string sentinel needed here.
 * @param {string | null | undefined} sdp_mid
 * @param {number} sdp_m_line_index
 * @param {string} candidate
 * @param {string} call_id
 * @returns {string | undefined}
 */
export function buildIcePayload(sdp_mid, sdp_m_line_index, candidate, call_id) {
    var ptr0 = isLikeNone(sdp_mid) ? 0 : passStringToWasm0(sdp_mid, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    var len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(candidate, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ptr2 = passStringToWasm0(call_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len2 = WASM_VECTOR_LEN;
    const ret = wasm.buildIcePayload(ptr0, len0, sdp_m_line_index, ptr1, len1, ptr2, len2);
    let v4;
    if (ret[0] !== 0) {
        v4 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v4;
}

/**
 * See [`crate::nostr_protocol::build_leaving_payload`]'s own doc.
 * @returns {string | undefined}
 */
export function buildLeavingPayload() {
    const ret = wasm.buildLeavingPayload();
    let v1;
    if (ret[0] !== 0) {
        v1 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v1;
}

/**
 * See [`crate::nostr_protocol::build_offer_payload`]'s own doc.
 * @param {string} sdp
 * @param {string} call_id
 * @returns {string | undefined}
 */
export function buildOfferPayload(sdp, call_id) {
    const ptr0 = passStringToWasm0(sdp, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(call_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.buildOfferPayload(ptr0, len0, ptr1, len1);
    let v3;
    if (ret[0] !== 0) {
        v3 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v3;
}

/**
 * See [`crate::nostr_protocol::build_relay_filters`]'s own doc. Returns a
 * JSON-encoded [`crate::nostr_protocol::RelayFilters`], falling back to
 * `{"wrap_filter":null,"bootstrap_filter":null}` on a panic — same
 * "nothing to filter for" no-op `nativeBuildRelayFilters` falls back to.
 * @param {string[]} confirmed_own_pubkeys
 * @param {string[]} pending_rendezvous_tags
 * @returns {string}
 */
export function buildRelayFilters(confirmed_own_pubkeys, pending_rendezvous_tags) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArrayJsValueToWasm0(confirmed_own_pubkeys, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayJsValueToWasm0(pending_rendezvous_tags, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.buildRelayFilters(ptr0, len0, ptr1, len1);
        deferred3_0 = ret[0];
        deferred3_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * See [`crate::nostr_protocol::build_wrapped_event`]'s own doc. Returns
 * the signed outer event as JSON, or `undefined` on any failure —
 * including a panic, matching `nativeBuildWrappedEvent`'s own convention.
 * @param {string} own_private_key_hex
 * @param {string} target_pubkey_hex
 * @param {string} payload_json
 * @returns {string | undefined}
 */
export function buildWrappedEvent(own_private_key_hex, target_pubkey_hex, payload_json) {
    const ptr0 = passStringToWasm0(own_private_key_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(target_pubkey_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ptr2 = passStringToWasm0(payload_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len2 = WASM_VECTOR_LEN;
    const ret = wasm.buildWrappedEvent(ptr0, len0, ptr1, len1, ptr2, len2);
    let v4;
    if (ret[0] !== 0) {
        v4 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v4;
}

/**
 * See [`crate::call_ui::phase`]'s own doc. Returns a JSON-encoded
 * [`crate::call_ui::PhaseView`]; a panic reads as "no call".
 * @param {boolean} has_active_call
 * @param {boolean} has_outcome
 * @param {boolean} has_ring
 * @param {boolean} accepted_incoming
 * @param {boolean} peer_connected
 * @param {boolean} peer_offline
 * @returns {string}
 */
export function callPhase(has_active_call, has_outcome, has_ring, accepted_incoming, peer_connected, peer_offline) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.callPhase(has_active_call, has_outcome, has_ring, accepted_incoming, peer_connected, peer_offline);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::call_arbitration::can_place_call`]'s own doc. `false` (no
 * Call button) on a panic — the fail-closed choice.
 * @param {boolean} connected
 * @returns {boolean}
 */
export function canPlaceCall(connected) {
    const ret = wasm.canPlaceCall(connected);
    return ret !== 0;
}

/**
 * See [`crate::cancel_attempt`]'s own doc. Returns a JSON array of effects (the signed cancel for the peer, if
 * the exchange had matched).
 * @param {string} pairing_id
 * @returns {string}
 */
export function cancelAttempt(pairing_id) {
    let deferred2_0;
    let deferred2_1;
    try {
        const ptr0 = passStringToWasm0(pairing_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.cancelAttempt(ptr0, len0);
        deferred2_0 = ret[0];
        deferred2_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
    }
}

/**
 * See [`crate::call_arbitration::check_call_timeout`]'s own doc. Falls back
 * to `[]` on a panic — matches `nativeCheckCallTimeout`'s own reasoning: a
 * failure to safely read state here doesn't warrant tearing down a call
 * that (for all this function could tell) may still be fine; the next tick
 * tries again. `now_ms` is `f64` — same `Date.now()` convention as every
 * other timestamp crossing this boundary.
 * @param {number} now_ms
 * @returns {string}
 */
export function checkCallTimeout(now_ms) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.checkCallTimeout(now_ms);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::presence::check_online_timeouts`]'s own doc. `now_ms`: see
 * [`mark_seen`]'s own doc for why `f64`, not `i64`.
 * @param {number} now_ms
 * @returns {string}
 */
export function checkOnlineTimeouts(now_ms) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.checkOnlineTimeouts(now_ms);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::presence::current_heartbeat_interval_ms`]'s own doc.
 * Returns `u32` directly (a plain JS `number`). Falls back to
 * [`crate::presence::HEARTBEAT_INTERVAL_MS`] (the slow, steady-state
 * cadence) on a panic, matching `nativeCurrentHeartbeatIntervalMs`'s own
 * fail-closed choice — a performance detail, not a correctness one.
 * @returns {number}
 */
export function currentHeartbeatIntervalMs() {
    const ret = wasm.currentHeartbeatIntervalMs();
    return ret >>> 0;
}

/**
 * See [`crate::presence::current_pairing_republish_interval_ms`]'s own doc.
 * 0 means no attempt is live. Falls back to 0 on a panic (the shell's pairing
 * timer just stops; the next pairing change starts it again).
 * @returns {number}
 */
export function currentPairingRepublishIntervalMs() {
    const ret = wasm.currentPairingRepublishIntervalMs();
    return ret >>> 0;
}

/**
 * See [`crate::signal_retry::due_for_retry`]'s own doc. Returns a
 * JSON-encoded `Vec<`[`crate::signal_retry::PendingRetry`]`>`, falling back
 * to `"[]"` on a panic — fail-closed, nothing to retry this tick.
 * @param {number} now_ms
 * @returns {string}
 */
export function dueForRetry(now_ms) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.dueForRetry(now_ms);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::relay_status::export_memory`]'s own doc. A JSON object; `"{}"`
 * on a panic.
 * @param {number} now_ms
 * @returns {string}
 */
export function exportRelayMemory(now_ms) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.exportRelayMemory(now_ms);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::call_arbitration::forget_pairing`]'s own doc. Same
 * always-a-JSON-array contract as the other effect-returning functions
 * above.
 * @param {string} pairing_id
 * @returns {string}
 */
export function forgetPairing(pairing_id) {
    let deferred2_0;
    let deferred2_1;
    try {
        const ptr0 = passStringToWasm0(pairing_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.forgetPairing(ptr0, len0);
        deferred2_0 = ret[0];
        deferred2_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
    }
}

/**
 * See [`crate::handle_timeout`]'s own doc. `generation` must be exactly
 * the value returned in `startAttempt`'s JSON result (see this module's
 * own doc for why it's `u32` here, not `u64`).
 * @param {string} pairing_id
 * @param {number} generation
 * @returns {string}
 */
export function handleTimeout(pairing_id, generation) {
    let deferred2_0;
    let deferred2_1;
    try {
        const ptr0 = passStringToWasm0(pairing_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.handleTimeout(ptr0, len0, generation);
        deferred2_0 = ret[0];
        deferred2_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
    }
}

/**
 * See [`crate::call_arbitration::hang_up`]'s own doc. Falls back to just
 * `[ClosePeerConnection]` on a panic — matches `nativeHangUp`'s own
 * reasoning: even if this module's own state couldn't be safely
 * read/cleared, the shell must still be told to tear down the real
 * `PeerConnection`, or a live call would leak.
 * @returns {string}
 */
export function hangUp() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.hangUp();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::ice_evidence::last_diagnosis`]'s own doc. Returns the
 * diagnosis's `snake_case` name (`"no_direct_path"`/`"udp_blocked"`), or
 * `undefined` for "can't tell" (including on a panic).
 * @returns {string | undefined}
 */
export function iceLastDiagnosis() {
    const ret = wasm.iceLastDiagnosis();
    let v1;
    if (ret[0] !== 0) {
        v1 = getStringFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v1;
}

/**
 * See [`crate::ice_evidence::note_connected`]'s own doc.
 */
export function iceNoteConnected() {
    wasm.iceNoteConnected();
}

/**
 * See [`crate::ice_evidence::note_local_candidate`]'s own doc.
 * `declared_type` is the browser's own `RTCIceCandidate.type` (`null`/
 * `undefined` when absent).
 * @param {string} candidate_line
 * @param {string | null} [declared_type]
 */
export function iceNoteLocalCandidate(candidate_line, declared_type) {
    const ptr0 = passStringToWasm0(candidate_line, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    var ptr1 = isLikeNone(declared_type) ? 0 : passStringToWasm0(declared_type, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    var len1 = WASM_VECTOR_LEN;
    wasm.iceNoteLocalCandidate(ptr0, len0, ptr1, len1);
}

/**
 * See [`crate::ice_evidence::note_remote_candidate`]'s own doc.
 * @param {string} candidate_line
 */
export function iceNoteRemoteCandidate(candidate_line) {
    const ptr0 = passStringToWasm0(candidate_line, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    wasm.iceNoteRemoteCandidate(ptr0, len0);
}

/**
 * See [`crate::ice_evidence::remember_diagnosis`]'s own doc.
 * @param {string} ice_connection_state
 */
export function iceRememberDiagnosis(ice_connection_state) {
    const ptr0 = passStringToWasm0(ice_connection_state, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    wasm.iceRememberDiagnosis(ptr0, len0);
}

/**
 * See [`crate::ice_evidence::reset`]'s own doc.
 */
export function iceReset() {
    wasm.iceReset();
}

/**
 * See [`crate::relay_status::import_memory`]'s own doc. No return value;
 * malformed input or a panic is a silent no-op.
 * @param {string} json
 * @param {number} now_ms
 */
export function importRelayMemory(json, now_ms) {
    const ptr0 = passStringToWasm0(json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    wasm.importRelayMemory(ptr0, len0, now_ms);
}

/**
 * See [`crate::call_arbitration::is_call_active`]'s own doc. Returns
 * `true` (fail toward *not* claiming to be free) on a panic — the shell
 * uses this to fill in its own outgoing heartbeat's busy field, and
 * wrongly broadcasting "not busy" while this module's own logic couldn't
 * be trusted is the worse direction to fail in.
 * @returns {boolean}
 */
export function isCallActive() {
    const ret = wasm.isCallActive();
    return ret !== 0;
}

/**
 * See [`crate::presence::is_online`]'s own doc. Returns `false` on a
 * panic, matching `nativeIsOnline`'s own "never seen" sentinel.
 * @param {string} pairing_id
 * @returns {boolean}
 */
export function isOnline(pairing_id) {
    const ptr0 = passStringToWasm0(pairing_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.isOnline(ptr0, len0);
    return ret !== 0;
}

/**
 * See [`crate::call_arbitration::mark_connected`]'s own doc. No return
 * value — pure bookkeeping, a panic here is swallowed the same way
 * `nativeMarkConnected` swallows one.
 * @param {string} pairing_id
 * @param {string} call_id
 */
export function markConnected(pairing_id, call_id) {
    const ptr0 = passStringToWasm0(pairing_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(call_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    wasm.markConnected(ptr0, len0, ptr1, len1);
}

/**
 * See [`crate::presence::mark_seen`]'s own doc. Returns a JSON-encoded
 * [`crate::presence::PresenceUpdateResult`], falling back to an empty one
 * on a panic (fail-closed: no UI transition, no call effect — matches
 * `nativeMarkSeen`'s own reasoning) — `now_ms` is `f64` (JS's own
 * `Date.now()` return type — `wasm-bindgen` maps `i64` to `BigInt`, which
 * `app.js` doesn't otherwise need to deal with anywhere else in this
 * module). `peer_busy`: `Option<bool>` crosses directly (`null`/`undefined`
 * from JS map straight to `None`, no JNI-style sentinel needed here) — see
 * `mark_seen`'s own doc for why this is `Some(_)` only for a `"heartbeat"`
 * message.
 * @param {string} pairing_id
 * @param {string} own_pubkey_hex
 * @param {string} peer_pubkey_hex
 * @param {number} now_ms
 * @param {boolean | null | undefined} peer_busy
 * @param {boolean} peer_hello
 * @returns {string}
 */
export function markSeen(pairing_id, own_pubkey_hex, peer_pubkey_hex, now_ms, peer_busy, peer_hello) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passStringToWasm0(pairing_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(own_pubkey_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(peer_pubkey_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.markSeen(ptr0, len0, ptr1, len1, ptr2, len2, now_ms, isLikeNone(peer_busy) ? 0xFFFFFF : peer_busy ? 1 : 0, peer_hello);
        deferred4_0 = ret[0];
        deferred4_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * See [`crate::call_arbitration::note_media_failure`]'s own doc.
 */
export function noteMediaFailure() {
    wasm.noteMediaFailure();
}

/**
 * See [`crate::relay_status::note_connect_error`]'s own doc.
 * @param {string} relay
 * @param {string} reason
 * @param {number} at_ms
 */
export function noteRelayConnectError(relay, reason, at_ms) {
    const ptr0 = passStringToWasm0(relay, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(reason, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    wasm.noteRelayConnectError(ptr0, len0, ptr1, len1, at_ms);
}

/**
 * See [`crate::relay_status::note_connected`]'s own doc.
 * @param {string} relay
 */
export function noteRelayConnected(relay) {
    const ptr0 = passStringToWasm0(relay, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    wasm.noteRelayConnected(ptr0, len0);
}

/**
 * See [`crate::relay_status::note_message`]'s own doc.
 * @param {string} relay
 * @param {number} now_ms
 */
export function noteRelayMessage(relay, now_ms) {
    const ptr0 = passStringToWasm0(relay, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    wasm.noteRelayMessage(ptr0, len0, now_ms);
}

/**
 * See [`crate::call_ui::outcome_text`]'s own doc. `reason`/`diagnosis` are
 * `snake_case` names (`diagnosis` empty or absent for none). Returns the
 * `snake_case` name of the [`crate::call_ui::OutcomeText`] case, falling
 * back to `never_connected`.
 * @param {string} reason
 * @param {string | null} [diagnosis]
 * @returns {string}
 */
export function outcomeText(reason, diagnosis) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(reason, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(diagnosis) ? 0 : passStringToWasm0(diagnosis, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        const ret = wasm.outcomeText(ptr0, len0, ptr1, len1);
        deferred3_0 = ret[0];
        deferred3_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * See [`crate::call_ui::pairing_phase`]'s own doc. Returns a JSON-encoded
 * [`crate::call_ui::PairingPhaseView`]; "preparing" on a panic.
 * @param {boolean} has_attempt
 * @param {number} relays_connected
 * @param {boolean} candidate_found
 * @param {boolean} accepted
 * @returns {string}
 */
export function pairingPhase(has_attempt, relays_connected, candidate_found, accepted) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.pairingPhase(has_attempt, relays_connected, candidate_found, accepted);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::call_arbitration::peer_connection_closed`]'s own doc.
 * **Now returns a JSON-encoded effects array** (was `void`) — see that
 * function's own doc for why (a genuinely spontaneous WebRTC teardown can
 * now carry a `ShowCallOutcome` down to the shell).
 * @returns {string}
 */
export function peerConnectionClosed() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.peerConnectionClosed();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::pending_attempt_ids`]'s own doc. Returns a JSON array of ids.
 * @returns {string}
 */
export function pendingAttemptIds() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.pendingAttemptIds();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::presence::remove_pairing`]'s own doc. No return value — a
 * panic here is swallowed the same way `nativePresenceRemovePairing`
 * swallows one.
 * @param {string} pairing_id
 */
export function presenceRemovePairing(pairing_id) {
    const ptr0 = passStringToWasm0(pairing_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    wasm.presenceRemovePairing(ptr0, len0);
}

/**
 * See [`crate::ProtocolConstants`]'s own doc. Call once at startup — these
 * never change at runtime — and use the result instead of a hand-copied
 * literal for `SIGNAL_KIND`/`WRAP_KIND`/the passphrase-pairing live-window
 * duration/name-length cap/SDP-and-ICE-candidate length caps. Falls back
 * to the same values re-encoded directly on a panic (this function can't
 * meaningfully fail otherwise — every field is a compile-time constant —
 * but every export in this file gets the same protection on principle).
 * @returns {string}
 */
export function protocolConstants() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.protocolConstants();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::signal_retry::record_pending_publish`]'s own doc. No return
 * value; a panic is a silent no-op, same reasoning as every other no-return export here.
 * @param {string} event_id
 * @param {string} event_json
 * @param {string} payload_json
 * @param {string[]} relays
 * @param {number} now_ms
 */
export function recordPendingPublish(event_id, event_json, payload_json, relays, now_ms) {
    const ptr0 = passStringToWasm0(event_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(event_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ptr2 = passStringToWasm0(payload_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len2 = WASM_VECTOR_LEN;
    const ptr3 = passArrayJsValueToWasm0(relays, wasm.__wbindgen_malloc);
    const len3 = WASM_VECTOR_LEN;
    wasm.recordPendingPublish(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3, now_ms);
}

/**
 * See [`crate::signal_retry::record_publish_result`]'s own doc. No return
 * value; a panic is a silent no-op.
 * @param {string} event_id
 * @param {string} relay
 * @param {boolean} accepted
 * @param {string} reason
 * @param {number} now_ms
 */
export function recordPublishResult(event_id, relay, accepted, reason, now_ms) {
    const ptr0 = passStringToWasm0(event_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(relay, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ptr2 = passStringToWasm0(reason, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len2 = WASM_VECTOR_LEN;
    wasm.recordPublishResult(ptr0, len0, ptr1, len1, accepted, ptr2, len2, now_ms);
}

/**
 * See [`crate::relay_list::apply`]'s own doc. Returns the JSON-encoded outcome.
 * @param {string} list_json
 * @param {number} now_ms
 * @returns {string}
 */
export function relayListApply(list_json, now_ms) {
    let deferred2_0;
    let deferred2_1;
    try {
        const ptr0 = passStringToWasm0(list_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.relayListApply(ptr0, len0, now_ms);
        deferred2_0 = ret[0];
        deferred2_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
    }
}

/**
 * See [`crate::relay_list::current`]'s own doc. Returns the JSON-encoded [`crate::relay_list::Current`].
 * @param {number} now_ms
 * @returns {string}
 */
export function relayListCurrent(now_ms) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.relayListCurrent(now_ms);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * See [`crate::relay_list::init`]'s own doc. `relays_json` is a JSON array of URLs.
 * @param {number} version
 * @param {string} relays_json
 */
export function relayListInit(version, relays_json) {
    const ptr0 = passStringToWasm0(relays_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    wasm.relayListInit(version, ptr0, len0);
}

/**
 * See [`crate::relay_status::view`]'s own doc. Returns a JSON array of
 * [`crate::relay_status::RelayView`]; `"[]"` on a panic.
 * @param {string[]} relays
 * @param {string[]} connected
 * @param {number} now_ms
 * @returns {string}
 */
export function relayView(relays, connected, now_ms) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArrayJsValueToWasm0(relays, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayJsValueToWasm0(connected, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.relayView(ptr0, len0, ptr1, len1, now_ms);
        deferred3_0 = ret[0];
        deferred3_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * See [`crate::call_arbitration::request_call`]'s own doc. Returns a
 * JSON-encoded [`crate::call_arbitration::RequestCallResult`], or a JS
 * exception on a panic (see [`start_attempt`]'s own doc for why that's
 * the right shape here, not a silent fallback value — this function's
 * only reachable from a local, already-validated UI action, never
 * directly from relay input).
 * @param {string} pairing_id
 * @param {string} own_pubkey_hex
 * @param {string} peer_pubkey_hex
 * @param {boolean} peer_online
 * @param {number} now_ms
 * @returns {string}
 */
export function requestCall(pairing_id, own_pubkey_hex, peer_pubkey_hex, peer_online, now_ms) {
    let deferred5_0;
    let deferred5_1;
    try {
        const ptr0 = passStringToWasm0(pairing_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(own_pubkey_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(peer_pubkey_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.requestCall(ptr0, len0, ptr1, len1, ptr2, len2, peer_online, now_ms);
        var ptr4 = ret[0];
        var len4 = ret[1];
        if (ret[3]) {
            ptr4 = 0; len4 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred5_0 = ptr4;
        deferred5_1 = len4;
        return getStringFromWasm0(ptr4, len4);
    } finally {
        wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
    }
}

/**
 * See [`crate::presence::request_hello`]'s own doc.
 */
export function requestHello() {
    wasm.requestHello();
}

/**
 * See [`crate::signal_router::route_event`]'s own doc. `context_json` is a
 * JSON-encoded [`crate::signal_router::RouteContext`]. Returns a JSON-encoded
 * [`crate::signal_router::RouteResult`]; a panic routes nothing.
 * @param {string} event_json
 * @param {string} context_json
 * @param {number} now_ms
 * @returns {string}
 */
export function routeEvent(event_json, context_json, now_ms) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(event_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(context_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.routeEvent(ptr0, len0, ptr1, len1, now_ms);
        deferred3_0 = ret[0];
        deferred3_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * See [`crate::sanitize_name`]'s own doc — exposed standalone too, so
 * `app.js`'s own `sanitizeName` (used for own-device name entry/rename,
 * not just the pairing flow) can be replaced by this single
 * implementation instead of keeping a second hand-copy around. Returns
 * the original string unchanged on a panic (fails open, deliberately,
 * unlike every other function in this file) — matches
 * `nativeSanitizeName`'s own identical reasoning: this is display
 * sanitization, not a security boundary on its own, and losing a human's
 * typed name entirely on an internal panic is worse than showing it
 * unsanitized.
 * @param {string} name
 * @returns {string}
 */
export function sanitizeName(name) {
    let deferred2_0;
    let deferred2_1;
    try {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sanitizeName(ptr0, len0);
        deferred2_0 = ret[0];
        deferred2_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
    }
}

/**
 * See [`crate::start_attempt`]'s own doc. Returns a JSON-encoded
 * [`crate::StartResult`], or a JS exception on a panic — matches
 * `nativeStartAttempt`'s `null`-on-panic in spirit (both are this
 * function's existing "something went wrong, nothing to hand back"
 * signal); an exception here is easy for `app.js` to `.catch()` the same
 * way it already handles this function's genuine `Err` path.
 * @param {string} own_name
 * @param {string} passphrase
 * @returns {string}
 */
export function startAttempt(own_name, passphrase) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passStringToWasm0(own_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(passphrase, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.startAttempt(ptr0, len0, ptr1, len1);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg___wbindgen_is_function_fcda5e3902d732fe: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_object_edb6b15aa3afe12e: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_string_c4f7cb494a2a21f1: function(arg0) {
            const ret = typeof(arg0) === 'string';
            return ret;
        },
        __wbg___wbindgen_is_undefined_8c687d0b90d5b524: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_string_get_92ab86bb19cbc12f: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_5d9e815e6fdf150f: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg_call_6bcf8d3e20937e46: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_crypto_38df2bab126b63dc: function(arg0) {
            const ret = arg0.crypto;
            return ret;
        },
        __wbg_getRandomValues_c44a50d8cfdaebeb: function() { return handleError(function (arg0, arg1) {
            arg0.getRandomValues(arg1);
        }, arguments); },
        __wbg_length_31bdaf014f5fbde2: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_msCrypto_bd5a034af96bcba6: function(arg0) {
            const ret = arg0.msCrypto;
            return ret;
        },
        __wbg_new_with_length_5ffeddb9d9fbb96f: function(arg0) {
            const ret = new Uint8Array(arg0 >>> 0);
            return ret;
        },
        __wbg_node_84ea875411254db1: function(arg0) {
            const ret = arg0.node;
            return ret;
        },
        __wbg_now_d1fb6650485d7f3e: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_process_44c7a14e11e9f69e: function(arg0) {
            const ret = arg0.process;
            return ret;
        },
        __wbg_prototypesetcall_ae9f5e7459250748: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_randomFillSync_6c25eac9869eb53c: function() { return handleError(function (arg0, arg1) {
            arg0.randomFillSync(arg1);
        }, arguments); },
        __wbg_require_b4edbdcf3e2a1ef0: function() { return handleError(function () {
            const ret = module.require;
            return ret;
        }, arguments); },
        __wbg_static_accessor_GLOBAL_8eb4cd83130a11a0: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_1e7044f654e934db: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_d8b50611246a6d92: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_fd0bc376bf0f8b42: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_subarray_1daff70dde20c145: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_versions_276b2795b1c6a219: function(arg0) {
            const ret = arg0.versions;
            return ret;
        },
        __wbindgen_generic_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U8)) -> NamedExternref("Uint8Array")`.
            const ret = getArrayU8FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_generic_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./call_core_bg.js": import0,
    };
}

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

function getArrayJsValueFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(wasm.__wbindgen_externrefs.get(mem.getUint32(i, true)));
    }
    wasm.__externref_drop_slice(ptr, len);
    return result;
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function passArrayJsValueToWasm0(array, malloc) {
    const ptr = malloc(array.length * 4, 4) >>> 0;
    for (let i = 0; i < array.length; i++) {
        const add = addToExternrefTable0(array[i]);
        getDataViewMemory0().setUint32(ptr + 4 * i, add, true);
    }
    WASM_VECTOR_LEN = array.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (!module.ok) {
            throw new Error(`failed to fetch Wasm: ${module.status} ${module.statusText} fetching '${module.url}'`);
        }

        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('call_core_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
