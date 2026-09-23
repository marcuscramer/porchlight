package dev.porchlight.app

import org.json.JSONArray
import org.json.JSONObject

/** JNI has no clean nullable primitive, so an `Option<bool>` crossing that
 * boundary (see [CallCoreBridge.markSeen]'s `peerBusy` param) uses the same
 * sentinel-value convention this project already applies to nullable
 * strings there (e.g. `sdpMid`'s empty-string-means-null): `-1` for `null`,
 * `0`/`1` for `false`/`true`. */
private fun Boolean?.toSentinelInt(): Int = when (this) {
    null -> -1
    false -> 0
    true -> 1
}

/**
 * Kotlin wrapper over the `call-core` Rust crate's pairing-bootstrap state
 * machine. Replaces `CameraAgentService`'s own hand-written
 * `PakeAttempt`/`onPairingBootstrapMessage`/`verifyPairingConfirmation`
 * logic — every decision about what a bootstrap message *means* now
 * happens in Rust; this class (and `CameraAgentService`, which owns the
 * actual instance of driving it) is purely the imperative shell: forward
 * events in, execute the [Effect]s that come back.
 *
 * Unlike `pake-bridge`'s handle-based `Session` design (needed there
 * because a SPAKE2 exchange has real per-call lifecycle to track), every
 * function here is a plain call keyed by `pairingId` — `call-core` owns
 * all attempt state internally, so there's no object on this side to hold
 * a handle to. Structured values cross as JSON, parsed here into small
 * Kotlin types.
 *
 * **Threading**: same rule as `pake-bridge` and everything else pairing/
 * call related — every call into this object must happen from
 * `CameraAgentService`'s `callExecutor` (see that class's own "Threading"
 * doc). The Rust side is internally `Mutex`-guarded regardless (cheap
 * insurance, not a substitute for this discipline), but effects returned
 * from one call are only meaningful if nothing else touches the same
 * `pairingId`'s state in between them being executed.
 */
object CallCoreBridge {
    init {
        System.loadLibrary("call_core")
    }

    @JvmStatic
    private external fun nativeStartAttempt(pairingId: String, ownPubkeyHex: String, ownName: String, passphrase: String): String?

    @JvmStatic
    private external fun nativeCancelAttempt(pairingId: String)

    @JvmStatic
    private external fun nativeBuildBootstrapPayload(pairingId: String): String?

    @JvmStatic
    private external fun nativeHandleBootstrapMessage(pairingId: String, senderPubkey: String, type: String, payloadJson: String): String

    @JvmStatic
    private external fun nativeHandleTimeout(pairingId: String, generation: Long): String

    @JvmStatic
    private external fun nativeSanitizeName(name: String): String?

    // --- Call arbitration (activePairingId/activeCallId/pendingOffer/
    // wantsCall in the old hand-written Kotlin) — see call-core's own
    // `call_arbitration` module doc for the full design. ---

    @JvmStatic
    private external fun nativeRequestCall(pairingId: String, ownPubkeyHex: String, peerPubkeyHex: String, peerOnline: Boolean): String?

    @JvmStatic
    private external fun nativeHandleShouldOffer(
        pairingId: String,
        callId: String,
        ownPubkeyHex: String,
        peerPubkeyHex: String,
        autoAnswer: Boolean,
    ): String

    @JvmStatic
    private external fun nativeHandleOffer(pairingId: String, callId: String, sdp: String, autoAnswer: Boolean): String

    @JvmStatic
    private external fun nativeShouldApplyAnswer(pairingId: String, callId: String): Boolean

    // sdpMid: "" means null (see call-core/src/android.rs's own note) —
    // sidesteps JNI's null-JString handling; a real ICE sdpMid is never
    // itself an empty string.
    @JvmStatic
    private external fun nativeHandleRemoteIce(pairingId: String, callId: String, sdpMid: String, sdpMLineIndex: Int, candidate: String): String

    @JvmStatic
    private external fun nativeAcceptIncomingCall(): String?

    @JvmStatic
    private external fun nativeTickIncomingCallCountdown(pairingId: String, callId: String): String

    @JvmStatic
    private external fun nativeHangUp(): String

    @JvmStatic
    private external fun nativeHandlePeerHangup(pairingId: String, callId: String): String

    @JvmStatic
    private external fun nativeHandlePeerBusy(pairingId: String, callId: String): String

    @JvmStatic
    private external fun nativePeerConnectionClosed(): String

    @JvmStatic
    private external fun nativeMarkConnected(pairingId: String, callId: String)

    @JvmStatic
    private external fun nativeShouldEndCallOnMediaFailure(hasActivePeerConnection: Boolean): Boolean

    @JvmStatic
    private external fun nativeForgetPairing(pairingId: String): String

    // --- Presence / adaptive heartbeat (lastSeenAt/onlineState/
    // pendingCreatedAt in the old hand-written Kotlin) — see call-core's
    // own `presence` module doc for the full design. `pendingPairingIds`/
    // `currentPendingIds` cross as a JSON array of strings (see
    // call-core/src/android.rs's own note on why: no native JNI string
    // array support, unlike wasm-bindgen's `Vec<String>` on the web side). ---

    // peerBusy: tri-state (-1/0/1 for unknown/false/true) — see the Rust
    // crate's own `nativeMarkSeen` doc for why JNI needs a sentinel here
    // instead of a real nullable boolean.
    @JvmStatic
    private external fun nativeMarkSeen(pairingId: String, ownPubkeyHex: String, peerPubkeyHex: String, nowMs: Long, peerBusy: Int): String

    @JvmStatic
    private external fun nativeHandleLeavingMessage(pairingId: String): String

    @JvmStatic
    private external fun nativeCheckOnlineTimeouts(nowMs: Long): String

    @JvmStatic
    private external fun nativeIsOnline(pairingId: String): Boolean

    @JvmStatic
    private external fun nativeHandlePeerBusyReply(pairingId: String, ownPubkeyHex: String, peerPubkeyHex: String, callId: String): String

    @JvmStatic
    private external fun nativeIsPeerBusy(pairingId: String): Boolean

    @JvmStatic
    private external fun nativeIsCallActive(): Boolean

    @JvmStatic
    private external fun nativeCurrentHeartbeatIntervalMs(pendingPairingIdsJson: String, nowMs: Long): Int

    @JvmStatic
    private external fun nativePruneStalePending(currentPendingIdsJson: String)

    @JvmStatic
    private external fun nativePresenceRemovePairing(pairingId: String)

    // --- Nostr protocol (gift-wrap construction/unwrapping, plain
    // bootstrap-event construction/verification, event dedup, relay
    // filter-set construction) — see call-core's own `nostr_protocol`
    // module doc for the full design. `targetPubkeyHex` (nullable on the
    // Kotlin side) crosses as a possibly-*empty* JString — "" means null,
    // same convention as ICE's `sdpMid` above — a real pubkey hex is never
    // itself an empty string. `confirmedOwnPubkeysJson`/
    // `pendingRendezvousTagsJson` are each a JSON array of strings, same
    // convention as presence's own `Vec<String>` params. ---

    @JvmStatic
    private external fun nativeBuildWrappedEvent(ownPrivateKeyHex: String, targetPubkeyHex: String, payloadJson: String): String?

    @JvmStatic
    private external fun nativeUnwrapWrappedEventForAny(wrapEventJson: String, candidatesJson: String): String?

    @JvmStatic
    private external fun nativeBuildBootstrapEvent(ownPrivateKeyHex: String, rendezvousTag: String, targetPubkeyHex: String, payloadJson: String): String?

    @JvmStatic
    private external fun nativeVerifyBootstrapEvent(eventJson: String): String?

    @JvmStatic
    private external fun nativeMarkSeenOrIsDuplicate(eventId: String): Boolean

    @JvmStatic
    private external fun nativeBuildRelayFilters(confirmedOwnPubkeysJson: String, pendingRendezvousTagsJson: String): String

    // --- Signal message schema (SignalMessage) — see call-core's own
    // `nostr_protocol::SignalMessage` doc. `sdpMid` crosses the same way
    // ICE's own `sdpMid` already does above: "" means null. ---

    @JvmStatic
    private external fun nativeBuildHeartbeatPayload(name: String, busy: Boolean): String?

    @JvmStatic
    private external fun nativeBuildLeavingPayload(): String?

    @JvmStatic
    private external fun nativeBuildByePayload(callId: String): String?

    @JvmStatic
    private external fun nativeBuildBusyPayload(callId: String): String?

    @JvmStatic
    private external fun nativeBuildCallPayload(callId: String): String?

    @JvmStatic
    private external fun nativeBuildOfferPayload(sdp: String, callId: String): String?

    @JvmStatic
    private external fun nativeBuildAnswerPayload(sdp: String, callId: String): String?

    @JvmStatic
    private external fun nativeBuildIcePayload(sdpMid: String, sdpMLineIndex: Int, candidate: String, callId: String): String?

    @JvmStatic
    private external fun nativeParseSignalPayload(payloadJson: String): String?

    @JvmStatic
    private external fun nativeProtocolConstants(): String?

    /** What starting a new attempt hands back — see the Rust crate's own
     * `StartResult` doc. [generation] must be passed back verbatim to
     * [handleTimeout] when the live window elapses. */
    data class StartResult(val rendezvousTag: String, val outboundHex: String, val generation: Long)

    /** Everything needed to (re)publish a heartbeat tick for one live
     * pending pairing, all at once — see the Rust crate's own
     * `PendingSnapshot` doc. */
    data class PendingSnapshot(val rendezvousTag: String, val payload: JSONObject, val candidatePubkey: String?)

    /** Effects the caller must actually perform — signaling sends, UI-facing
     * contact-state updates, timer kicks. Mirrors the Rust crate's `Effect`
     * enum one-for-one; see its own doc for what each means. */
    sealed interface Effect {
        data class SendBootstrap(val pairingId: String, val rendezvousTag: String, val targetPubkey: String, val payload: JSONObject) : Effect
        data object KickHeartbeat : Effect
        data class SetCollision(val pairingId: String) : Effect
        data class SetTimedOut(val pairingId: String) : Effect
        data class SetConfirmedCandidate(val pairingId: String, val pubkeyHex: String, val name: String) : Effect
    }

    /** Effects the caller must actually perform for the call-arbitration
     * state machine — mirrors the Rust crate's `call_arbitration::CallEffect`
     * enum one-for-one; see its own doc for what each means. Deliberately no
     * Android-only variants (wake lock/foreground bring-up) — this class's
     * own effect handler does that extra work itself whenever it sees
     * [CallEffect.StartRinging] or the tie-break fast-path's
     * [CallEffect.ApplyRemoteOffer]. */
    sealed interface CallEffect {
        data object AcquireMedia : CallEffect
        data class CreateOffer(val pairingId: String, val callId: String) : CallEffect
        data class SendCall(val pairingId: String, val callId: String) : CallEffect
        data class SendBusy(val pairingId: String, val callId: String) : CallEffect
        data class ApplyRemoteOffer(val pairingId: String, val callId: String, val sdp: String) : CallEffect
        data class StartRinging(val pairingId: String, val callId: String, val autoAnswer: Boolean, val secondsRemaining: Int) : CallEffect
        data class SendBye(val pairingId: String, val callId: String) : CallEffect
        /** A terminal call needs a full-screen message shown — see the Rust
         * crate's own `CallEffect::ShowCallOutcome`/`CallOutcomeReason` doc
         * for exactly which teardown paths emit this and why (not every
         * one does: a local hang-up or a busy reply never does). */
        data class ShowCallOutcome(val pairingId: String, val callId: String, val reason: CallOutcomeReason) : CallEffect
        data object ClosePeerConnection : CallEffect
        data object ClearIncomingCallTimer : CallEffect
    }

    enum class CallOutcomeReason { PEER_ENDED, NEVER_CONNECTED, DROPPED }

    /** What [requestCall] hands back — see the Rust crate's own
     * `RequestCallResult` doc. [callId] is `null` only when a *different*
     * pairing already owns the call slot (a silent no-op — no [CallEffect]
     * either). */
    data class RequestCallResult(val callId: String?, val effects: List<CallEffect>)

    data class IceCandidateData(val sdpMid: String?, val sdpMLineIndex: Int, val candidate: String)

    /** What accepting a ring actually means to do next — see the Rust
     * crate's own `AcceptOutcome` doc for why this is two variants, not one
     * flat result with an optional `sdp`: conflating them used to mean a
     * `"call"` message's recipient winning the pubkey tie-break connected
     * with no ring at all (see [handleShouldOffer]'s own doc). */
    sealed interface AcceptOutcome {
        data class ApplyOffer(val pairingId: String, val callId: String, val sdp: String, val iceBuffer: List<IceCandidateData>) : AcceptOutcome
        data class CreateOffer(val pairingId: String, val callId: String) : AcceptOutcome
    }

    /** See the Rust crate's own `IceOutcome` doc. */
    sealed interface IceOutcome {
        data object Buffered : IceOutcome
        data class Apply(val sdpMid: String?, val sdpMLineIndex: Int, val candidate: String) : IceOutcome
        data object Dropped : IceOutcome
    }

    /** See the Rust crate's own `TickOutcome` doc. */
    sealed interface TickOutcome {
        data object Stale : TickOutcome
        data class Continue(val secondsRemaining: Int) : TickOutcome
        data object ShouldAccept : TickOutcome
    }

    /** A pairing's presence, from this device's own point of view — mirrors
     * the Rust crate's own `presence::PresenceStatus` doc: exactly one of
     * three values at any moment, never a combination. Used to be two
     * independent booleans on this side too (`ContactState.online`/`.busy`)
     * that could disagree. */
    enum class PresenceStatus { OFFLINE, ONLINE, BUSY }

    /** What the caller should do about a pairing's presence UI — mirrors
     * the Rust crate's own `presence::PresenceEffect` doc: one effect
     * covering all three states, not separate online/busy effects. Android
     * additionally clears `connected` on `PresenceStatus.OFFLINE` (matching
     * this platform's own `ContactState`) — see [NostrSignalingClient]'s
     * `onPresenceUpdate` implementer for that. */
    sealed interface PresenceEffect {
        data class SetStatus(val pairingId: String, val status: PresenceStatus) : PresenceEffect
    }

    /** What every presence-mutating call hands back — see the Rust crate's
     * own `presence::PresenceUpdateResult` doc for why these two effect
     * lists are fused into one result. */
    data class PresenceUpdateResult(val presenceEffects: List<PresenceEffect>, val callEffects: List<CallEffect>)

    /** Starts a new pairing attempt for [pairingId], replacing any existing
     * one under the same id. Trim/NFC-normalization and the rendezvous-tag
     * derivation happen inside `pake-bridge`, called from `call-core` — see
     * that crate's own doc; this call forwards the raw typed passphrase
     * unchanged. */
    fun startAttempt(pairingId: String, ownPubkeyHex: String, ownName: String, passphrase: String): StartResult {
        val json = checkNotNull(nativeStartAttempt(pairingId, ownPubkeyHex, ownName, passphrase)) {
            "CallCoreBridge.startAttempt failed at the native layer"
        }
        val obj = JSONObject(json)
        return StartResult(obj.getString("rendezvous_tag"), obj.getString("outbound_hex"), obj.getLong("generation"))
    }

    /** Abandons an attempt outright (user cancel, contact deleted) without
     * completing it. A no-op if there's no live attempt for [pairingId]. */
    fun cancelAttempt(pairingId: String) = nativeCancelAttempt(pairingId)

    /** Everything needed to (re)publish a heartbeat tick for one live
     * pending pairing — rendezvous tag, current payload (`pake1` or
     * `pake-confirm` depending on how far the exchange has gotten), and
     * the candidate's pubkey once known. `null` if there's no live attempt
     * for [pairingId]. */
    fun pendingSnapshot(pairingId: String): PendingSnapshot? {
        val json = nativeBuildBootstrapPayload(pairingId) ?: return null
        val obj = JSONObject(json)
        return PendingSnapshot(
            rendezvousTag = obj.getString("rendezvous_tag"),
            payload = obj.getJSONObject("payload"),
            candidatePubkey = if (obj.isNull("candidate_pubkey")) null else obj.getString("candidate_pubkey"),
        )
    }

    /** Drives the entire SPAKE2 pairing state machine for one incoming
     * bootstrap message — see the Rust crate's own doc. [type] and
     * [payload] mirror the wire message's own shape exactly (as already
     * parsed by the caller from the relay event). */
    fun handleBootstrapMessage(pairingId: String, senderPubkey: String, type: String, payload: JSONObject): List<Effect> =
        parseEffects(nativeHandleBootstrapMessage(pairingId, senderPubkey, type, payload.toString()))

    /** The live-window timeout fired for [pairingId] — see the Rust crate's
     * own doc for why [generation] (from [StartResult]) matters: a no-op
     * unless it's still the same, unresolved attempt this timeout was
     * scheduled for. */
    fun handleTimeout(pairingId: String, generation: Long): List<Effect> = parseEffects(nativeHandleTimeout(pairingId, generation))

    /** Strips control/bidi-override/zero-width characters from a
     * self-reported display name — see the Rust crate's own doc. Falls
     * back to the original [name] unchanged if the native call somehow
     * fails, rather than losing what a human typed. */
    fun sanitizeName(name: String): String = nativeSanitizeName(name) ?: name

    // --- Call arbitration ---

    /** Starts a call attempt — see the Rust crate's own `request_call` doc
     * for the full design (unifies what used to be split across
     * `CameraAgentService.requestCall` and
     * `NostrSignalingClient.requestCall`'s pubkey tie-break). [peerOnline]
     * is supplied by the caller's own presence tracking (unmoved — still
     * `NostrSignalingClient`'s territory). */
    fun requestCall(pairingId: String, ownPubkeyHex: String, peerPubkeyHex: String, peerOnline: Boolean): RequestCallResult {
        val json = checkNotNull(nativeRequestCall(pairingId, ownPubkeyHex, peerPubkeyHex, peerOnline)) {
            "CallCoreBridge.requestCall failed at the native layer"
        }
        val obj = JSONObject(json)
        return RequestCallResult(
            callId = if (obj.isNull("call_id")) null else obj.getString("call_id"),
            effects = parseCallEffects(obj.getJSONArray("effects").toString()),
        )
    }

    /** See the Rust crate's own `handle_should_offer` doc — the pubkey
     * tie-break check now lives inside that function itself, so the caller
     * just forwards both pubkeys through unconditionally like every other
     * message type already does. [autoAnswer] is looked up by the caller
     * (its own contacts list), same as [handleOffer]. */
    fun handleShouldOffer(pairingId: String, callId: String, ownPubkeyHex: String, peerPubkeyHex: String, autoAnswer: Boolean): List<CallEffect> =
        parseCallEffects(nativeHandleShouldOffer(pairingId, callId, ownPubkeyHex, peerPubkeyHex, autoAnswer))

    /** See the Rust crate's own `handle_offer` doc. [autoAnswer] is looked
     * up by the caller (its own contacts list) — `call-core` doesn't own
     * contacts. No longer takes a `hasActivePeerConnection` flag: dropped
     * from the Rust signature once found to be provably redundant with
     * `offer_applied`, state `call-core` already owned itself. */
    fun handleOffer(pairingId: String, callId: String, sdp: String, autoAnswer: Boolean): List<CallEffect> =
        parseCallEffects(nativeHandleOffer(pairingId, callId, sdp, autoAnswer))

    /** See the Rust crate's own `should_apply_answer` doc — `true` at most
     * once per call; a redelivered answer returns `false`. The real WebRTC
     * `signalingState` check stays a *second*, independent guard inside
     * [WebRtcEngine.handleRemoteAnswer], not replaced by this. */
    fun shouldApplyAnswer(pairingId: String, callId: String): Boolean = nativeShouldApplyAnswer(pairingId, callId)

    /** See the Rust crate's own `handle_remote_ice` doc. */
    fun handleRemoteIce(pairingId: String, callId: String, sdpMid: String?, sdpMLineIndex: Int, candidate: String): IceOutcome =
        parseIceOutcome(nativeHandleRemoteIce(pairingId, callId, sdpMid ?: "", sdpMLineIndex, candidate))

    /** `null` if there's no live pending ring (e.g. a stray UI tap after
     * the call already resolved some other way) — see the Rust crate's own
     * `accept_incoming_call`/`AcceptOutcome` docs for the two shapes this
     * can come back as. */
    fun acceptIncomingCall(): AcceptOutcome? {
        val json = nativeAcceptIncomingCall() ?: return null
        val obj = JSONObject(json)
        return when (val kind = obj.getString("kind")) {
            "ApplyOffer" -> {
                val iceArray = obj.getJSONArray("ice_buffer")
                val iceBuffer = (0 until iceArray.length()).map { i ->
                    val iceObj = iceArray.getJSONObject(i)
                    IceCandidateData(
                        sdpMid = if (iceObj.isNull("sdp_mid")) null else iceObj.getString("sdp_mid"),
                        sdpMLineIndex = iceObj.getInt("sdp_m_line_index"),
                        candidate = iceObj.getString("candidate"),
                    )
                }
                AcceptOutcome.ApplyOffer(obj.getString("pairing_id"), obj.getString("call_id"), obj.getString("sdp"), iceBuffer)
            }
            "CreateOffer" -> AcceptOutcome.CreateOffer(obj.getString("pairing_id"), obj.getString("call_id"))
            else -> error("CallCoreBridge: unknown accept outcome kind from native layer: $kind")
        }
    }

    /** One tick of the auto-answer countdown — see the Rust crate's own
     * `tick_incoming_call_countdown` doc. The caller owns the actual timer
     * loop: call this once, one second after [CallEffect.StartRinging]
     * arrived, and again one second after every [TickOutcome.Continue]. */
    fun tickIncomingCallCountdown(pairingId: String, callId: String): TickOutcome = parseTickOutcome(nativeTickIncomingCallCountdown(pairingId, callId))

    /** The invariant #4 function — see the Rust crate's own `hang_up` doc
     * for why this is structurally safe against the historical
     * read-after-teardown bug. */
    fun hangUp(): List<CallEffect> = parseCallEffects(nativeHangUp())

    /** See the Rust crate's own `handle_peer_hangup` doc. */
    fun handlePeerHangup(pairingId: String, callId: String): List<CallEffect> = parseCallEffects(nativeHandlePeerHangup(pairingId, callId))

    /** See the Rust crate's own `handle_peer_busy` doc. */
    fun handlePeerBusy(pairingId: String, callId: String): List<CallEffect> = parseCallEffects(nativeHandlePeerBusy(pairingId, callId))

    /** Call any time a real `PeerConnection` is noticed gone, for *any*
     * reason — see the Rust crate's own `peer_connection_closed` doc for
     * why this is a separate event from [CallEffect.ClosePeerConnection],
     * not just its natural consequence. Returns effects now (was `Unit`) —
     * may carry a [CallEffect.ShowCallOutcome] when this is a genuinely
     * spontaneous teardown call-core never otherwise decided on; `[]` when
     * some other function (hangUp/handlePeerHangup/…) already fully
     * decided this same teardown — see that Rust doc for the exactly-once
     * guarantee. */
    fun peerConnectionClosed(): List<CallEffect> = parseCallEffects(nativePeerConnectionClosed())

    /** Called the instant the real `PeerConnection` reaches CONNECTED —
     * see the Rust crate's own `mark_connected` doc. */
    fun markConnected(pairingId: String, callId: String) = nativeMarkConnected(pairingId, callId)

    /** See the Rust crate's own `should_end_call_on_media_failure` doc. */
    fun shouldEndCallOnMediaFailure(hasActivePeerConnection: Boolean): Boolean = nativeShouldEndCallOnMediaFailure(hasActivePeerConnection)

    /** See the Rust crate's own `call_arbitration::forget_pairing` doc —
     * call alongside [presenceRemovePairing] from `removePairing`. */
    fun forgetPairing(pairingId: String): List<CallEffect> = parseCallEffects(nativeForgetPairing(pairingId))

    // --- Presence / adaptive heartbeat ---

    /** Mirrors `markSeen`+`setOnline`'s online branch fused together — see
     * the Rust crate's own `presence::mark_seen` doc. Call unconditionally
     * for every dispatched message from a confirmed peer, before even
     * looking at its type. [peerBusy] is the sender's own self-reported
     * "am I on a call" status — `null` for every message type except
     * `"heartbeat"` (the only payload that actually carries this field);
     * pass whatever was parsed (or wasn't found) straight through rather
     * than branching on message type at the call site. */
    fun markSeen(pairingId: String, ownPubkeyHex: String, peerPubkeyHex: String, nowMs: Long, peerBusy: Boolean?): PresenceUpdateResult =
        parsePresenceUpdateResult(nativeMarkSeen(pairingId, ownPubkeyHex, peerPubkeyHex, nowMs, peerBusy.toSentinelInt()))

    /** See the Rust crate's own `presence::handle_leaving_message` doc —
     * call for the `"leaving"` wire-message case. */
    fun handleLeavingMessage(pairingId: String): PresenceUpdateResult = parsePresenceUpdateResult(nativeHandleLeavingMessage(pairingId))

    /** Mirrors receiving a `"busy"` reply to our own outgoing call attempt
     * — see the Rust crate's own `presence::handle_peer_busy_reply` doc.
     * Fuses what used to be two separate calls this class had to remember
     * to make together (mark the peer busy immediately, and release this
     * device's own claimed call slot) into one. [ownPubkeyHex]/
     * [peerPubkeyHex]: the same tie-break inputs [markSeen] takes — this
     * call now performs its own online transition rather than assuming the
     * caller already ran [markSeen] for this same message (see the Rust
     * doc for why that assumption was a real gap, not just style). */
    fun handlePeerBusyReply(pairingId: String, ownPubkeyHex: String, peerPubkeyHex: String, callId: String): PresenceUpdateResult =
        parsePresenceUpdateResult(nativeHandlePeerBusyReply(pairingId, ownPubkeyHex, peerPubkeyHex, callId))

    /** See the Rust crate's own `presence::is_peer_busy` doc — used to seed
     * a contact's initial UI state (e.g. after [restartAgent] rebuilds the
     * contact list from scratch; this module's own state is a process-
     * global Rust static that survives that, same reasoning as [isOnline]'s
     * existing use here). */
    fun isPeerBusy(pairingId: String): Boolean = nativeIsPeerBusy(pairingId)

    /** See the Rust crate's own `call_arbitration::is_call_active` doc —
     * what every outgoing heartbeat now reports as this device's own busy
     * status. */
    fun isCallActive(): Boolean = nativeIsCallActive()

    /** See the Rust crate's own `presence::check_online_timeouts` doc. Call
     * once per sweep tick (this class's own polling cadence, not owned by
     * `call-core`). */
    fun checkOnlineTimeouts(nowMs: Long): PresenceUpdateResult = parsePresenceUpdateResult(nativeCheckOnlineTimeouts(nowMs))

    /** See the Rust crate's own `presence::is_online` doc — `false`, not
     * "unknown," for a pairing never seen at all. */
    fun isOnline(pairingId: String): Boolean = nativeIsOnline(pairingId)

    /** See the Rust crate's own `presence::current_heartbeat_interval_ms`
     * doc, including its side effect of seeding `pending_created_at` the
     * first time a given pending id is seen. */
    fun currentHeartbeatIntervalMs(pendingPairingIds: List<String>, nowMs: Long): Int =
        nativeCurrentHeartbeatIntervalMs(JSONArray(pendingPairingIds).toString(), nowMs)

    /** See the Rust crate's own `presence::prune_stale_pending` doc — call
     * at the top of `heartbeatTick`, same as today. */
    fun pruneStalePending(currentPendingIds: List<String>) = nativePruneStalePending(JSONArray(currentPendingIds).toString())

    /** See the Rust crate's own `presence::remove_pairing` doc — call
     * alongside [forgetPairing] from `removePairing`. */
    fun presenceRemovePairing(pairingId: String) = nativePresenceRemovePairing(pairingId)

    // --- Nostr protocol ---

    /** Everything a verified bootstrap event hands back — see the Rust
     * crate's own `nostr_protocol::VerifiedBootstrapEvent` doc. */
    data class VerifiedBootstrapEvent(val senderPubkeyHex: String, val rendezvousTag: String, val payloadJson: String)

    /** One relay filter's worth of data — see the Rust crate's own
     * `nostr_protocol::FilterSpec` doc for why this is deliberately not a
     * richer `Filter` type: the caller translates this into whatever its
     * own relay-client library's own filter type expects. */
    data class FilterSpec(val kind: Int, val tagName: Char, val tagValues: List<String>)

    /** See the Rust crate's own `nostr_protocol::RelayFilters` doc.
     * Either half is `null` when there's nothing to filter for yet. */
    data class RelayFilters(val wrapFilter: FilterSpec?, val bootstrapFilter: FilterSpec?)

    /** Mirrors `publish`'s full two-layer gift wrap exactly — see the Rust
     * crate's own `nostr_protocol::build_wrapped_event` doc. Returns the
     * signed outer event as JSON, ready to hand to the caller's own
     * relay-pool `publish()` call, or `null` on any failure (malformed
     * hex, or a panic). */
    fun buildWrappedEvent(ownPrivateKeyHex: String, targetPubkeyHex: String, payloadJson: String): String? =
        nativeBuildWrappedEvent(ownPrivateKeyHex, targetPubkeyHex, payloadJson)

    /** One confirmed pairing's routing-and-decryption keys — see the Rust
     * crate's own `nostr_protocol::WrapEventCandidate` doc, including for
     * why [lastSignalCreatedAt]/[lastSignalEventId] (mirror
     * [Config.Pairing.lastSignalCreatedAt]/[Config.Pairing.lastSignalEventId])
     * are here now — *both*, not just the timestamp: Nostr `created_at` is
     * whole seconds, so two different, legitimately back-to-back messages
     * (a `"call"` immediately followed by its own `"offer"`) routinely
     * share one, and only the id actually disambiguates them. */
    data class WrapEventCandidate(
        val pairingId: String,
        val ownPrivateKeyHex: String,
        val peerPublicKey: String,
        val lastSignalCreatedAt: Long,
        val lastSignalEventId: String,
    )

    /** What a successfully routed-and-decrypted wrap event hands back —
     * see the Rust crate's own `nostr_protocol::RoutedSignalPayload` doc.
     * [signalCreatedAt]/[signalEventId]: the caller must persist both as
     * that pairing's new `lastSignalCreatedAt`/`lastSignalEventId` (see
     * those fields' own doc). */
    data class RoutedSignalPayload(val pairingId: String, val payloadJson: String, val signalCreatedAt: Long, val signalEventId: String)

    /** Mirrors `handleWrapEvent`'s full `p`-tag routing + verify-decrypt-
     * verify-decrypt chain — see the Rust crate's own
     * `nostr_protocol::unwrap_wrapped_event_for_any` doc. Replaces this
     * class's own hand-rolled `event.tags.find { it[0] == "p" }` +
     * `confirmedPeers.find { ownPubkeyHexFor(...) == recipientPubkeyHex }`
     * linear search — the caller now just hands over every confirmed
     * peer's own routing/decryption keys and gets back which one (if any)
     * actually matched, already decrypted. `null` if the event doesn't
     * route to any of [candidates], is strictly older than (or the exact
     * same event as) that candidate's own
     * [WrapEventCandidate.lastSignalCreatedAt]/[WrapEventCandidate.lastSignalEventId]
     * (a relay redelivering something already processed — see that field's
     * own doc), fails to decrypt/verify once routed, or a panic. */
    fun unwrapWrappedEventForAny(wrapEventJson: String, candidates: List<WrapEventCandidate>): RoutedSignalPayload? {
        val candidatesJson = JSONArray(
            candidates.map {
                JSONObject()
                    .put("pairing_id", it.pairingId)
                    .put("own_private_key_hex", it.ownPrivateKeyHex)
                    .put("peer_public_key", it.peerPublicKey)
                    .put("last_signal_created_at", it.lastSignalCreatedAt)
                    .put("last_signal_event_id", it.lastSignalEventId)
            },
        )
        val json = nativeUnwrapWrappedEventForAny(wrapEventJson, candidatesJson.toString()) ?: return null
        val obj = JSONObject(json)
        return RoutedSignalPayload(obj.getString("pairing_id"), obj.getString("payload_json"), obj.getLong("signal_created_at"), obj.getString("signal_event_id"))
    }

    /** One signaling message exchanged between confirmed peers — see the
     * Rust crate's own `nostr_protocol::SignalMessage` doc. Used to be six
     * independently hand-built `JSONObject`s on the send side
     * (`NostrSignalingClient.hangUp`/`sendBusy`/`sendCall`/`sendOffer`/
     * `sendAnswer`/`sendIce`) and one hand-parsed `when` on the receive
     * side (`dispatchFromConfirmedPeer`'s own per-type extraction) — both
     * now go through [buildHeartbeatPayload]/etc. and [parseSignalPayload]
     * instead. */
    sealed interface SignalMessage {
        data class Heartbeat(val name: String, val busy: Boolean?) : SignalMessage
        data object Leaving : SignalMessage
        data class Bye(val callId: String) : SignalMessage
        data class Busy(val callId: String) : SignalMessage
        data class Call(val callId: String) : SignalMessage
        data class Offer(val sdp: String, val callId: String) : SignalMessage
        data class Answer(val sdp: String, val callId: String) : SignalMessage
        data class Ice(val sdpMid: String?, val sdpMLineIndex: Int, val candidate: String, val callId: String) : SignalMessage
    }

    /** Mirrors `publishBootstrap`'s plain (unencrypted) self-signed event —
     * see the Rust crate's own `nostr_protocol::build_bootstrap_event` doc.
     * [targetPubkeyHex] is `null` until the candidate's own pubkey is
     * known. `null` return on any failure. */
    fun buildBootstrapEvent(ownPrivateKeyHex: String, rendezvousTag: String, targetPubkeyHex: String?, payloadJson: String): String? =
        nativeBuildBootstrapEvent(ownPrivateKeyHex, rendezvousTag, targetPubkeyHex ?: "", payloadJson)

    /** Mirrors `handleBootstrapEvent`'s verification exactly — see the
     * Rust crate's own `nostr_protocol::verify_bootstrap_event` doc. `null`
     * if the signature is invalid, there's no `d` tag, or a panic. */
    fun verifyBootstrapEvent(eventJson: String): VerifiedBootstrapEvent? {
        val json = nativeVerifyBootstrapEvent(eventJson) ?: return null
        val obj = JSONObject(json)
        return VerifiedBootstrapEvent(obj.getString("sender_pubkey_hex"), obj.getString("rendezvous_tag"), obj.getString("payload_json"))
    }

    /** The `seenEventIds` dedup — see the Rust crate's own
     * `nostr_protocol::mark_seen_or_is_duplicate` doc. `true` for a
     * genuinely new [eventId] (process it); `false` if already seen (drop
     * it silently). Falls back to `true` on a panic — see that function's
     * own doc for why this one deliberately fails *open*. */
    fun markSeenOrIsDuplicate(eventId: String): Boolean = nativeMarkSeenOrIsDuplicate(eventId)

    /** Mirrors `currentFilters`'s exact filter-set construction — see the
     * Rust crate's own `nostr_protocol::build_relay_filters` doc. */
    fun buildRelayFilters(confirmedOwnPubkeys: List<String>, pendingRendezvousTags: List<String>): RelayFilters {
        val json = nativeBuildRelayFilters(JSONArray(confirmedOwnPubkeys).toString(), JSONArray(pendingRendezvousTags).toString())
        return parseRelayFilters(json)
    }

    /** Mirrors `heartbeatTick`'s payload — see the Rust crate's own
     * `nostr_protocol::build_heartbeat_payload` doc. */
    fun buildHeartbeatPayload(name: String, busy: Boolean): String? = nativeBuildHeartbeatPayload(name, busy)

    /** Mirrors `close()`'s `sendToConfirmedPeer(peer, "leaving")` — see the
     * Rust crate's own `nostr_protocol::build_leaving_payload` doc. */
    fun buildLeavingPayload(): String? = nativeBuildLeavingPayload()

    /** Mirrors `hangUp`'s payload — see the Rust crate's own
     * `nostr_protocol::build_bye_payload` doc. */
    fun buildByePayload(callId: String): String? = nativeBuildByePayload(callId)

    /** Mirrors `sendBusy`'s payload — see the Rust crate's own
     * `nostr_protocol::build_busy_payload` doc. */
    fun buildBusyPayload(callId: String): String? = nativeBuildBusyPayload(callId)

    /** Mirrors `sendCall`'s payload — see the Rust crate's own
     * `nostr_protocol::build_call_payload` doc. */
    fun buildCallPayload(callId: String): String? = nativeBuildCallPayload(callId)

    /** Mirrors `sendOffer`'s payload — see the Rust crate's own
     * `nostr_protocol::build_offer_payload` doc. */
    fun buildOfferPayload(sdp: String, callId: String): String? = nativeBuildOfferPayload(sdp, callId)

    /** Mirrors `sendAnswer`'s payload — see the Rust crate's own
     * `nostr_protocol::build_answer_payload` doc. */
    fun buildAnswerPayload(sdp: String, callId: String): String? = nativeBuildAnswerPayload(sdp, callId)

    /** Mirrors `sendIce`'s payload — see the Rust crate's own
     * `nostr_protocol::build_ice_payload` doc. */
    fun buildIcePayload(sdpMid: String?, sdpMLineIndex: Int, candidate: String, callId: String): String? =
        nativeBuildIcePayload(sdpMid ?: "", sdpMLineIndex, candidate, callId)

    /** Mirrors `dispatchFromConfirmedPeer`'s full per-type extraction and
     * validation — see the Rust crate's own `nostr_protocol::parse_signal_payload`
     * doc. `null` for an unrecognized type, a malformed shape, a blank/
     * oversized `sdp`/`candidate`, or a panic — the caller drops the
     * message silently, same as every other message this device can't
     * safely interpret. A `heartbeat`'s [SignalMessage.Heartbeat.name]
     * comes back already capped and sanitized — the caller no longer needs
     * to do either itself. */
    fun parseSignalPayload(payloadJson: String): SignalMessage? {
        val json = nativeParseSignalPayload(payloadJson) ?: return null
        val obj = JSONObject(json)
        return when (val type = obj.getString("type")) {
            "heartbeat" -> SignalMessage.Heartbeat(obj.getString("name"), if (obj.has("busy")) obj.getBoolean("busy") else null)
            "leaving" -> SignalMessage.Leaving
            "bye" -> SignalMessage.Bye(obj.getString("callId"))
            "busy" -> SignalMessage.Busy(obj.getString("callId"))
            "call" -> SignalMessage.Call(obj.getString("callId"))
            "offer" -> SignalMessage.Offer(obj.getString("sdp"), obj.getString("callId"))
            "answer" -> SignalMessage.Answer(obj.getString("sdp"), obj.getString("callId"))
            "ice" -> SignalMessage.Ice(
                if (obj.has("sdpMid")) obj.getString("sdpMid") else null,
                obj.optInt("sdpMLineIndex"),
                obj.getString("candidate"),
                obj.getString("callId"),
            )
            else -> error("CallCoreBridge: unknown signal message type from native layer: $type")
        }
    }

    /** Protocol-level constants both platforms must use identically — see
     * the Rust crate's own `ProtocolConstants` doc. Read this instead of
     * hand-copying a literal wherever one of these values is needed. */
    data class ProtocolConstants(
        val signalKind: Int,
        val wrapKind: Int,
        val maxNameLength: Int,
        val pakeLiveWindowMs: Long,
        val maxSdpLength: Int,
        val maxIceCandidateLength: Int,
    )

    /** Lazy, not called at class-init time: the native library isn't
     * guaranteed loaded yet the moment this object is first referenced.
     * Cached rather than re-fetched per call since these are compile-time
     * constants on the Rust side. Falls back to the same values re-declared
     * directly on a panic or malformed JSON — a safety net, not a second
     * source of truth to keep in sync. */
    val protocolConstants: ProtocolConstants by lazy {
        val json = nativeProtocolConstants()
        if (json == null) {
            ProtocolConstants(signalKind = 20331, wrapKind = 20336, maxNameLength = 100, pakeLiveWindowMs = 120_000L, maxSdpLength = 65_536, maxIceCandidateLength = 4_096)
        } else {
            val obj = JSONObject(json)
            ProtocolConstants(
                signalKind = obj.getInt("signal_kind"),
                wrapKind = obj.getInt("wrap_kind"),
                maxNameLength = obj.getInt("max_name_length"),
                pakeLiveWindowMs = obj.getLong("pake_live_window_ms"),
                maxSdpLength = obj.getInt("max_sdp_length"),
                maxIceCandidateLength = obj.getInt("max_ice_candidate_length"),
            )
        }
    }

    private fun parseFilterSpec(obj: JSONObject?): FilterSpec? {
        if (obj == null) return null
        val tagValuesArray = obj.getJSONArray("tag_values")
        val tagValues = (0 until tagValuesArray.length()).map { tagValuesArray.getString(it) }
        return FilterSpec(obj.getInt("kind"), obj.getString("tag_name")[0], tagValues)
    }

    private fun parseRelayFilters(json: String): RelayFilters {
        val obj = JSONObject(json)
        return RelayFilters(
            wrapFilter = parseFilterSpec(if (obj.isNull("wrap_filter")) null else obj.getJSONObject("wrap_filter")),
            bootstrapFilter = parseFilterSpec(if (obj.isNull("bootstrap_filter")) null else obj.getJSONObject("bootstrap_filter")),
        )
    }

    private fun parseEffects(json: String): List<Effect> {
        val array = JSONArray(json)
        return (0 until array.length()).map { i ->
            val obj = array.getJSONObject(i)
            when (val kind = obj.getString("kind")) {
                "SendBootstrap" -> Effect.SendBootstrap(obj.getString("pairing_id"), obj.getString("rendezvous_tag"), obj.getString("target_pubkey"), obj.getJSONObject("payload"))
                "KickHeartbeat" -> Effect.KickHeartbeat
                "SetCollision" -> Effect.SetCollision(obj.getString("pairing_id"))
                "SetTimedOut" -> Effect.SetTimedOut(obj.getString("pairing_id"))
                "SetConfirmedCandidate" -> Effect.SetConfirmedCandidate(obj.getString("pairing_id"), obj.getString("pubkey_hex"), obj.getString("name"))
                else -> error("CallCoreBridge: unknown effect kind from native layer: $kind")
            }
        }
    }

    private fun parseCallEffects(json: String): List<CallEffect> {
        val array = JSONArray(json)
        return (0 until array.length()).map { i ->
            val obj = array.getJSONObject(i)
            when (val kind = obj.getString("kind")) {
                "AcquireMedia" -> CallEffect.AcquireMedia
                "CreateOffer" -> CallEffect.CreateOffer(obj.getString("pairing_id"), obj.getString("call_id"))
                "SendCall" -> CallEffect.SendCall(obj.getString("pairing_id"), obj.getString("call_id"))
                "SendBusy" -> CallEffect.SendBusy(obj.getString("pairing_id"), obj.getString("call_id"))
                "ApplyRemoteOffer" -> CallEffect.ApplyRemoteOffer(obj.getString("pairing_id"), obj.getString("call_id"), obj.getString("sdp"))
                "StartRinging" -> CallEffect.StartRinging(obj.getString("pairing_id"), obj.getString("call_id"), obj.getBoolean("auto_answer"), obj.getInt("seconds_remaining"))
                "SendBye" -> CallEffect.SendBye(obj.getString("pairing_id"), obj.getString("call_id"))
                "ShowCallOutcome" -> CallEffect.ShowCallOutcome(
                    obj.getString("pairing_id"),
                    obj.getString("call_id"),
                    CallOutcomeReason.valueOf(obj.getString("reason").uppercase()),
                )
                "ClosePeerConnection" -> CallEffect.ClosePeerConnection
                "ClearIncomingCallTimer" -> CallEffect.ClearIncomingCallTimer
                else -> error("CallCoreBridge: unknown call effect kind from native layer: $kind")
            }
        }
    }

    private fun parseIceOutcome(json: String): IceOutcome {
        val obj = JSONObject(json)
        return when (val outcome = obj.getString("outcome")) {
            "Buffered" -> IceOutcome.Buffered
            "Apply" -> IceOutcome.Apply(if (obj.isNull("sdp_mid")) null else obj.getString("sdp_mid"), obj.getInt("sdp_m_line_index"), obj.getString("candidate"))
            "Dropped" -> IceOutcome.Dropped
            else -> error("CallCoreBridge: unknown ice outcome from native layer: $outcome")
        }
    }

    private fun parseTickOutcome(json: String): TickOutcome {
        val obj = JSONObject(json)
        return when (val outcome = obj.getString("outcome")) {
            "Stale" -> TickOutcome.Stale
            "Continue" -> TickOutcome.Continue(obj.getInt("seconds_remaining"))
            "ShouldAccept" -> TickOutcome.ShouldAccept
            else -> error("CallCoreBridge: unknown tick outcome from native layer: $outcome")
        }
    }

    private fun parsePresenceUpdateResult(json: String): PresenceUpdateResult {
        val obj = JSONObject(json)
        val presenceArray = obj.getJSONArray("presence_effects")
        val presenceEffects = (0 until presenceArray.length()).map { i ->
            val effectObj = presenceArray.getJSONObject(i)
            when (val kind = effectObj.getString("kind")) {
                "SetStatus" -> PresenceEffect.SetStatus(effectObj.getString("pairing_id"), PresenceStatus.valueOf(effectObj.getString("status").uppercase()))
                else -> error("CallCoreBridge: unknown presence effect kind from native layer: $kind")
            }
        }
        return PresenceUpdateResult(presenceEffects, parseCallEffects(obj.getJSONArray("call_effects").toString()))
    }
}
