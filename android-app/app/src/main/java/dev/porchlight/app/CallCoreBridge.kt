package dev.porchlight.app

import org.json.JSONArray
import org.json.JSONObject

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
    private external fun nativeHandleTimeout(pairingId: String, generation: Long): String

    @JvmStatic
    private external fun nativeSanitizeName(name: String): String?

    // --- Call arbitration (activePairingId/activeCallId/pendingOffer/
    // wantsCall in the old hand-written Kotlin) — see call-core's own
    // `call_arbitration` module doc for the full design. ---

    @JvmStatic
    private external fun nativeRequestCall(pairingId: String, ownPubkeyHex: String, peerPubkeyHex: String, peerOnline: Boolean, nowMs: Long): String?

    @JvmStatic
    private external fun nativeAcceptIncomingCall(nowMs: Long): String?

    @JvmStatic
    private external fun nativeTickIncomingCallCountdown(pairingId: String, callId: String): String

    @JvmStatic
    private external fun nativeHangUp(): String

    @JvmStatic
    private external fun nativeCheckCallTimeout(nowMs: Long): String

    @JvmStatic
    private external fun nativePeerConnectionClosed(): String

    @JvmStatic
    private external fun nativeNoteMediaFailure()

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

    @JvmStatic
    private external fun nativeRouteEvent(eventJson: String, contextJson: String, nowMs: Long): String

    @JvmStatic
    private external fun nativeRequestHello()

    @JvmStatic
    private external fun nativeCheckOnlineTimeouts(nowMs: Long): String

    @JvmStatic
    private external fun nativeIsOnline(pairingId: String): Boolean

    @JvmStatic
    private external fun nativeIsPeerBusy(pairingId: String): Boolean

    @JvmStatic
    private external fun nativeIsCallActive(): Boolean

    @JvmStatic
    private external fun nativeCurrentHeartbeatIntervalMs(pendingPairingIdsJson: String): Int

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
    private external fun nativeBuildBootstrapEvent(ownPrivateKeyHex: String, rendezvousTag: String, targetPubkeyHex: String, payloadJson: String): String?

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
    private external fun nativeProtocolConstants(): String?

    @JvmStatic
    private external fun nativeWakeUpStart(serviceEnabled: Boolean, switchOn: Boolean): String

    @JvmStatic
    private external fun nativeWakeUpNextStep(elapsedMs: Long, presses: Int, interactive: Boolean, uiResumed: Boolean, dreaming: Boolean): String

    @JvmStatic
    private external fun nativeWakeUpConstants(): String

    @JvmStatic
    private external fun nativeCanPlaceCall(isPaired: Boolean, connected: Boolean): Boolean

    @JvmStatic
    private external fun nativeIceReset()

    @JvmStatic
    private external fun nativeIceNoteLocalCandidate(candidateLine: String, declaredType: String)

    @JvmStatic
    private external fun nativeIceNoteRemoteCandidate(candidateLine: String)

    @JvmStatic
    private external fun nativeIceNoteConnected()

    @JvmStatic
    private external fun nativeIceRememberDiagnosis(iceConnectionState: String)

    @JvmStatic
    private external fun nativeCallPhase(hasActiveCall: Boolean, hasOutcome: Boolean, hasRing: Boolean, acceptedIncoming: Boolean, peerConnected: Boolean): String

    @JvmStatic
    private external fun nativeOutcomeText(reason: String, diagnosis: String): String

    @JvmStatic
    private external fun nativeIceLastDiagnosis(): String?

    @JvmStatic
    private external fun nativeRecordPendingPublish(eventId: String, eventJson: String, payloadJson: String, relaysJson: String, nowMs: Long)

    @JvmStatic
    private external fun nativeRecordPublishResult(eventId: String, relay: String, accepted: Boolean, reason: String, nowMs: Long)

    @JvmStatic
    private external fun nativeAvailableRelays(candidatesJson: String, nowMs: Long): String

    @JvmStatic
    private external fun nativeDueForRetry(nowMs: Long): String

    @JvmStatic
    private external fun nativeExportRelayMemory(nowMs: Long): String

    @JvmStatic
    private external fun nativeImportRelayMemory(json: String, nowMs: Long)

    @JvmStatic
    private external fun nativeNoteRelayMessage(relay: String, nowMs: Long)

    @JvmStatic
    private external fun nativeNoteRelayConnected(relay: String)

    @JvmStatic
    private external fun nativeNoteRelayConnectError(relay: String, reason: String, atMs: Long)

    @JvmStatic
    private external fun nativeRelayView(relaysJson: String, connectedJson: String, nowMs: Long): String

    @JvmStatic
    private external fun nativeAgo(atMs: Long, nowMs: Long): String

    /** What starting a new attempt hands back — see the Rust crate's own
     * `StartResult` doc. [generation] must be passed back verbatim to
     * [handleTimeout] when the live window elapses. */
    data class StartResult(val rendezvousTag: String, val outboundHex: String, val generation: Long)

    /** Everything needed to (re)publish a heartbeat tick for one live
     * pending pairing, all at once — see the Rust crate's own
     * `PendingSnapshot` doc. */
    data class PendingSnapshot(val rendezvousTag: String, val payloads: List<JSONObject>, val candidatePubkey: String?)

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
        /** Send a heartbeat now — see the Rust `CallEffect::KickHeartbeat` doc. */
        data object KickHeartbeat : CallEffect
    }

    enum class CallOutcomeReason { PEER_ENDED, DECLINED, CANCELLED, NO_ANSWER, UNREACHABLE, BUSY, CAMERA_FAILED, NEVER_CONNECTED, DROPPED }

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
        /** Send [pairingId]'s peer one ordinary heartbeat right now — see the
         * Rust crate's own `presence::PresenceEffect::ReplyHeartbeat` doc.
         * Handled by [NostrSignalingClient] itself, never forwarded. */
        data class ReplyHeartbeat(val pairingId: String) : PresenceEffect
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
     * pending pairing — rendezvous tag, the messages to publish (always
     * `pake1`, plus `pake-confirm` once a candidate's `pake1` has been
     * processed — see the Rust `build_bootstrap_payload` doc for why both),
     * and the candidate's pubkey once known. `null` if there's no live attempt
     * for [pairingId]. */
    fun pendingSnapshot(pairingId: String): PendingSnapshot? {
        val json = nativeBuildBootstrapPayload(pairingId) ?: return null
        val obj = JSONObject(json)
        return PendingSnapshot(
            rendezvousTag = obj.getString("rendezvous_tag"),
            payloads = obj.getJSONArray("payloads").let { arr -> (0 until arr.length()).map { arr.getJSONObject(it) } },
            candidatePubkey = if (obj.isNull("candidate_pubkey")) null else obj.getString("candidate_pubkey"),
        )
    }

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
    fun requestCall(pairingId: String, ownPubkeyHex: String, peerPubkeyHex: String, peerOnline: Boolean, nowMs: Long): RequestCallResult {
        val json = checkNotNull(nativeRequestCall(pairingId, ownPubkeyHex, peerPubkeyHex, peerOnline, nowMs)) {
            "CallCoreBridge.requestCall failed at the native layer"
        }
        val obj = JSONObject(json)
        return RequestCallResult(
            callId = if (obj.isNull("call_id")) null else obj.getString("call_id"),
            effects = parseCallEffects(obj.getJSONArray("effects").toString()),
        )
    }

    /** `null` if there's no live pending ring (e.g. a stray UI tap after
     * the call already resolved some other way) — see the Rust crate's own
     * `accept_incoming_call`/`AcceptOutcome` docs for the two shapes this
     * can come back as. */
    fun acceptIncomingCall(nowMs: Long): AcceptOutcome? {
        val json = nativeAcceptIncomingCall(nowMs) ?: return null
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

    /** See the Rust crate's own `check_call_timeout` doc — poll this on the
     * same periodic tick that already drives [checkOnlineTimeouts]. */
    fun checkCallTimeout(nowMs: Long): List<CallEffect> = parseCallEffects(nativeCheckCallTimeout(nowMs))

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

    /** This device's own camera/microphone failed: call before closing the
     * connection so the outcome says so (see the Rust crate's own
     * `note_media_failure` doc). */
    fun noteMediaFailure() = nativeNoteMediaFailure()

    /** Called the instant the real `PeerConnection` reaches CONNECTED —
     * see the Rust crate's own `mark_connected` doc. */
    fun markConnected(pairingId: String, callId: String) = nativeMarkConnected(pairingId, callId)

    /** See the Rust crate's own `should_end_call_on_media_failure` doc. */
    fun shouldEndCallOnMediaFailure(hasActivePeerConnection: Boolean): Boolean = nativeShouldEndCallOnMediaFailure(hasActivePeerConnection)

    /** See the Rust crate's own `call_arbitration::forget_pairing` doc —
     * call alongside [presenceRemovePairing] from `removePairing`. */
    fun forgetPairing(pairingId: String): List<CallEffect> = parseCallEffects(nativeForgetPairing(pairingId))

    // --- Presence / adaptive heartbeat ---

    /** See the Rust crate's own `presence::request_hello` doc: the next
     * heartbeat built asks every peer for an immediate reply. Call when
     * signaling connectivity returns. */
    fun requestHello() = nativeRequestHello()

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
     * doc: the delay until the next heartbeat tick. */
    fun currentHeartbeatIntervalMs(pendingPairingIds: List<String>): Int =
        nativeCurrentHeartbeatIntervalMs(JSONArray(pendingPairingIds).toString())

    /** See the Rust crate's own `presence::remove_pairing` doc — call
     * alongside [forgetPairing] from `removePairing`. */
    fun presenceRemovePairing(pairingId: String) = nativePresenceRemovePairing(pairingId)

    // --- Nostr protocol ---

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

    /** One confirmed pairing as the router needs it — see the Rust crate's
     * own `signal_router::RoutePeer`. [autoAnswer] is the contact's setting
     * at this moment. */
    data class RoutePeer(
        val pairingId: String,
        val ownPrivateKeyHex: String,
        val peerPublicKey: String,
        val lastSignalCreatedAt: Long,
        val lastSignalEventId: String,
        val autoAnswer: Boolean,
    )

    /** A pairing attempt still in progress, found by its rendezvous tag. */
    data class RoutePending(val pairingId: String, val rendezvousTag: String)

    /** The pairing's new "last processed signal" — persist it before doing
     * anything else with the result. */
    data class ProcessedSignal(val pairingId: String, val createdAt: Long, val eventId: String)

    /** What the shell does with a routed event beyond presence and call
     * effects — see the Rust crate's own `signal_router::RouteAction`. */
    sealed interface RouteAction {
        data class SendHeartbeat(val pairingId: String, val payloadJson: String) : RouteAction
        data class UpdatePeerName(val pairingId: String, val name: String) : RouteAction
        data class ApplyRemoteAnswer(val pairingId: String, val sdp: String) : RouteAction
        data class AddRemoteIce(val pairingId: String, val sdpMid: String?, val sdpMLineIndex: Int, val candidate: String) : RouteAction
    }

    /** The ordered result of one relay event: persist [signal], apply
     * [update], run [actions], then [bootstrapEffects]. */
    data class RouteResult(
        val signal: ProcessedSignal?,
        val update: PresenceUpdateResult,
        val actions: List<RouteAction>,
        val bootstrapEffects: List<Effect>,
    )

    /** The single entry point for an incoming relay event: dedup, unwrap and
     * verify, parse, presence and per-message handling all happen in
     * `call-core`'s `signal_router`. [deviceName] answers a peer's `hello`. */
    fun routeEvent(eventJson: String, deviceName: String, confirmed: List<RoutePeer>, pending: List<RoutePending>, nowMs: Long): RouteResult {
        val context = JSONObject()
            .put("device_name", deviceName)
            .put(
                "confirmed",
                JSONArray(
                    confirmed.map {
                        JSONObject()
                            .put("pairing_id", it.pairingId)
                            .put("own_private_key_hex", it.ownPrivateKeyHex)
                            .put("peer_public_key", it.peerPublicKey)
                            .put("last_signal_created_at", it.lastSignalCreatedAt)
                            .put("last_signal_event_id", it.lastSignalEventId)
                            .put("auto_answer", it.autoAnswer)
                    },
                ),
            )
            .put("pending", JSONArray(pending.map { JSONObject().put("pairing_id", it.pairingId).put("rendezvous_tag", it.rendezvousTag) }))
        return parseRouteResult(nativeRouteEvent(eventJson, context.toString(), nowMs))
    }

    /** Mirrors `publishBootstrap`'s plain (unencrypted) self-signed event —
     * see the Rust crate's own `nostr_protocol::build_bootstrap_event` doc.
     * [targetPubkeyHex] is `null` until the candidate's own pubkey is
     * known. `null` return on any failure. */
    fun buildBootstrapEvent(ownPrivateKeyHex: String, rendezvousTag: String, targetPubkeyHex: String?, payloadJson: String): String? =
        nativeBuildBootstrapEvent(ownPrivateKeyHex, rendezvousTag, targetPubkeyHex ?: "", payloadJson)

    /** See the Rust crate's own `call_arbitration::can_place_call` doc —
     * whether a contact's Call button is offered. */
    fun canPlaceCall(isPaired: Boolean, connected: Boolean): Boolean = nativeCanPlaceCall(isPaired, connected)

    // --- ICE evidence / call-failure diagnosis (Rust `ice_evidence`) -----------

    /** Why a call that never connected most likely failed at the network
     * level — see the Rust crate's own `ice_evidence` doc. */
    enum class IceDiagnosis { NO_DIRECT_PATH, UDP_BLOCKED }

    /** See the Rust crate's own `call_ui::Phase` doc. */
    enum class Phase { IDLE, OUTCOME, RINGING, CALLING, CONNECTING, LIVE }

    /** What the call screens show — see the Rust crate's own
     * `call_ui::PhaseView` doc. The label is picked per [phase] from the
     * app's own strings. */
    data class PhaseView(val phase: Phase, val showAccept: Boolean, val controlsPinned: Boolean)

    /** Which phase a call is in, decided once in `call-core` for both shells. */
    fun callPhase(hasActiveCall: Boolean, hasOutcome: Boolean, hasRing: Boolean, acceptedIncoming: Boolean, peerConnected: Boolean): PhaseView {
        val obj = JSONObject(nativeCallPhase(hasActiveCall, hasOutcome, hasRing, acceptedIncoming, peerConnected))
        return PhaseView(Phase.valueOf(obj.getString("phase").uppercase()), obj.getBoolean("show_accept"), obj.getBoolean("controls_pinned"))
    }

    /** Which text an ended call gets — see the Rust crate's own
     * `call_ui::OutcomeText` doc. */
    enum class OutcomeText { PEER_ENDED, DECLINED, CANCELLED, NO_ANSWER, UNREACHABLE, BUSY, CAMERA_FAILED, NEVER_CONNECTED, UDP_BLOCKED, NO_DIRECT_PATH, DROPPED }

    fun outcomeText(reason: CallOutcomeReason, diagnosis: IceDiagnosis?): OutcomeText =
        OutcomeText.valueOf(nativeOutcomeText(reason.name.lowercase(), diagnosis?.name?.lowercase() ?: "").trim('"').uppercase())

    /** A new `PeerConnection` begins: forget the previous one's evidence. */
    fun iceReset() = nativeIceReset()

    /** One of this device's own candidates was gathered. */
    fun iceNoteLocalCandidate(candidateLine: String) = nativeIceNoteLocalCandidate(candidateLine, "")

    /** A candidate arrived from the peer. */
    fun iceNoteRemoteCandidate(candidateLine: String) = nativeIceNoteRemoteCandidate(candidateLine)

    /** The connection reached CONNECTED at least once. */
    fun iceNoteConnected() = nativeIceNoteConnected()

    /** Diagnose from the evidence so far (given the connection's current ICE
     * state name, e.g. `CHECKING`) and keep the result for [iceLastDiagnosis]. */
    fun iceRememberDiagnosis(iceConnectionState: String) = nativeIceRememberDiagnosis(iceConnectionState)

    /** What [iceRememberDiagnosis] last concluded; `null` means "can't tell". */
    fun iceLastDiagnosis(): IceDiagnosis? = when (nativeIceLastDiagnosis()) {
        "no_direct_path" -> IceDiagnosis.NO_DIRECT_PATH
        "udp_blocked" -> IceDiagnosis.UDP_BLOCKED
        else -> null
    }

    /** One publish [dueForRetry] says to resend — see the Rust crate's own
     * `signal_retry::PendingRetry` doc. [eventJson] is exactly what was
     * passed to [recordPendingPublish]; [relays] is whichever of its
     * targets are still outstanding. */
    data class PendingRetry(val eventJson: String, val relays: List<String>)

    /** See the Rust crate's own `signal_retry::record_pending_publish` doc:
     * call right after the real publish attempt, with whichever [relays]
     * (plain URL strings) are already known to have missed it, and the plain
     * [payloadJson] the event wraps (call-core decides from it whether this
     * kind of message is worth retrying at all). */
    fun recordPendingPublish(eventId: String, eventJson: String, payloadJson: String, relays: Collection<String>, nowMs: Long) =
        nativeRecordPendingPublish(eventId, eventJson, payloadJson, JSONArray(relays).toString(), nowMs)

    /** See the Rust crate's own `signal_retry::record_publish_result` doc:
     * one relay's real outcome for [eventId]; [reason] is the relay's own
     * text for a rejection (empty is fine). */
    fun recordPublishResult(eventId: String, relay: String, accepted: Boolean, reason: String, nowMs: Long) =
        nativeRecordPublishResult(eventId, relay, accepted, reason, nowMs)

    /** See the Rust crate's own `signal_retry::available_relays` doc: which
     * of [candidates] a first publish should go to (the rest are cooling
     * down from a recent rejection). */
    fun availableRelays(candidates: Collection<String>, nowMs: Long): List<String> {
        val arr = JSONArray(nativeAvailableRelays(JSONArray(candidates).toString(), nowMs))
        return (0 until arr.length()).map { arr.getString(it) }
    }

    /** See the Rust crate's own `signal_retry::due_for_retry` doc: call from
     * the existing periodic tick, then republish each entry to its
     * [PendingRetry.relays]. */
    fun dueForRetry(nowMs: Long): List<PendingRetry> {
        val arr = JSONArray(nativeDueForRetry(nowMs))
        return (0 until arr.length()).map { i ->
            val obj = arr.getJSONObject(i)
            val relays = obj.getJSONArray("relays")
            PendingRetry(obj.getString("event_json"), (0 until relays.length()).map { relays.getString(it) })
        }
    }

    /** See the Rust crate's own `relay_status::export_memory` doc: the relay
     * cooldowns still running and each relay's last rejection, as JSON to
     * keep across restarts. */
    fun exportRelayMemory(nowMs: Long): String = nativeExportRelayMemory(nowMs)

    /** Restores what [exportRelayMemory] returned. */
    fun importRelayMemory(json: String, nowMs: Long) = nativeImportRelayMemory(json, nowMs)

    fun noteRelayMessage(relay: String, nowMs: Long) = nativeNoteRelayMessage(relay, nowMs)

    fun noteRelayConnected(relay: String) = nativeNoteRelayConnected(relay)

    fun noteRelayConnectError(relay: String, reason: String, atMs: Long) = nativeNoteRelayConnectError(relay, reason, atMs)

    /** See the Rust crate's own `relay_status::Ago` doc. */
    sealed interface Ago {
        data object Never : Ago
        data class Seconds(val n: Long) : Ago
        data class Minutes(val n: Long) : Ago
        data class Hours(val n: Long) : Ago
    }

    private fun parseAgo(obj: JSONObject): Ago = when (val unit = obj.getString("unit")) {
        "never" -> Ago.Never
        "seconds" -> Ago.Seconds(obj.getLong("n"))
        "minutes" -> Ago.Minutes(obj.getLong("n"))
        "hours" -> Ago.Hours(obj.getLong("n"))
        else -> error("CallCoreBridge: unknown ago unit from native layer: $unit")
    }

    /** How long ago [atMs] was ([null] → never). */
    fun ago(atMs: Long?, nowMs: Long): Ago = parseAgo(JSONObject(nativeAgo(atMs ?: -1, nowMs)))

    /** What the dot says: green connected, yellow paused after a rejection, red down. */
    enum class RelayState { CONNECTED, PAUSED, DOWN }

    /** One relay as the Status screen shows it — see the Rust
     * crate's own `relay_status::RelayView` doc. */
    data class RelayView(val url: String, val host: String, val state: RelayState, val accepted: Int, val rejected: Int, val error: String?, val ago: Ago)

    fun relayView(relays: List<String>, connected: Collection<String>, nowMs: Long): List<RelayView> {
        val array = JSONArray(nativeRelayView(JSONArray(relays).toString(), JSONArray(connected).toString(), nowMs))
        return (0 until array.length()).map { i ->
            val o = array.getJSONObject(i)
            RelayView(
                o.getString("url"),
                o.getString("host"),
                RelayState.valueOf(o.getString("state").uppercase()),
                o.getInt("accepted"),
                o.getInt("rejected"),
                if (o.isNull("error")) null else o.getString("error"),
                parseAgo(o.getJSONObject("ago")),
            )
        }
    }

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

    /** The next thing the call wake-up should do — see the Rust crate's own
     * `wake_up::Step` doc. All timing and branching decisions live there. */
    sealed interface WakeUpStep {
        data class WaitForScreen(val recheckAfterMs: Long) : WakeUpStep
        data object PlainBringToFront : WakeUpStep
        data class PressHome(val pressNumber: Int, val bringBackAfterMs: Long, val verifyAfterMs: Long) : WakeUpStep
        data class BringBack(val recheckAfterMs: Long) : WakeUpStep
        data object Settled : WakeUpStep
        data object GiveUp : WakeUpStep
    }

    /** True if a ringing call should run the Home-press steps (accessibility
     * service on *and* the Settings switch on), false for a plain
     * bring-to-front. */
    fun wakeUpShouldEscalate(serviceEnabled: Boolean, switchOn: Boolean): Boolean =
        JSONObject(nativeWakeUpStart(serviceEnabled, switchOn)).getString("kind") == "Escalate"

    /** See the Rust crate's own `wake_up::next_step` doc. [elapsedMs] is the
     * time since the ring started; [presses] the Home presses made so far. */
    fun wakeUpNextStep(elapsedMs: Long, presses: Int, interactive: Boolean, uiResumed: Boolean, dreaming: Boolean): WakeUpStep {
        val obj = JSONObject(nativeWakeUpNextStep(elapsedMs, presses, interactive, uiResumed, dreaming))
        return when (val kind = obj.getString("kind")) {
            "WaitForScreen" -> WakeUpStep.WaitForScreen(obj.getLong("recheck_after_ms"))
            "PlainBringToFront" -> WakeUpStep.PlainBringToFront
            "PressHome" -> WakeUpStep.PressHome(obj.getInt("press_number"), obj.getLong("bring_back_after_ms"), obj.getLong("verify_after_ms"))
            "BringBack" -> WakeUpStep.BringBack(obj.getLong("recheck_after_ms"))
            "Settled" -> WakeUpStep.Settled
            "GiveUp" -> WakeUpStep.GiveUp
            else -> error("CallCoreBridge: unknown wake-up step from native layer: $kind")
        }
    }

    /** The two wake-up timings a shell needs outside the step results. */
    data class WakeUpConstants(val ownPressWindowMs: Long, val maskTimeoutMs: Long)

    val wakeUpConstants: WakeUpConstants by lazy {
        val obj = JSONObject(nativeWakeUpConstants())
        WakeUpConstants(obj.getLong("own_press_window_ms"), obj.getLong("mask_timeout_ms"))
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
                "KickHeartbeat" -> CallEffect.KickHeartbeat
                else -> error("CallCoreBridge: unknown call effect kind from native layer: $kind")
            }
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

    private fun parseRouteResult(json: String): RouteResult {
        val obj = JSONObject(json)
        val signal = if (obj.isNull("signal")) null else obj.getJSONObject("signal").let { ProcessedSignal(it.getString("pairing_id"), it.getLong("created_at"), it.getString("event_id")) }
        val actionArray = obj.getJSONArray("actions")
        val actions = (0 until actionArray.length()).map { i ->
            val a = actionArray.getJSONObject(i)
            when (val kind = a.getString("kind")) {
                "SendHeartbeat" -> RouteAction.SendHeartbeat(a.getString("pairing_id"), a.getString("payload_json"))
                "UpdatePeerName" -> RouteAction.UpdatePeerName(a.getString("pairing_id"), a.getString("name"))
                "ApplyRemoteAnswer" -> RouteAction.ApplyRemoteAnswer(a.getString("pairing_id"), a.getString("sdp"))
                "AddRemoteIce" -> RouteAction.AddRemoteIce(
                    a.getString("pairing_id"),
                    if (a.isNull("sdp_mid")) null else a.getString("sdp_mid"),
                    a.getInt("sdp_m_line_index"),
                    a.getString("candidate"),
                )
                else -> error("CallCoreBridge: unknown route action kind from native layer: $kind")
            }
        }
        return RouteResult(signal, parsePresenceUpdateResult(json), actions, parseEffects(obj.getJSONArray("bootstrap_effects").toString()))
    }

    private fun parsePresenceUpdateResult(json: String): PresenceUpdateResult {
        val obj = JSONObject(json)
        val presenceArray = obj.getJSONArray("presence_effects")
        val presenceEffects = (0 until presenceArray.length()).map { i ->
            val effectObj = presenceArray.getJSONObject(i)
            when (val kind = effectObj.getString("kind")) {
                "SetStatus" -> PresenceEffect.SetStatus(effectObj.getString("pairing_id"), PresenceStatus.valueOf(effectObj.getString("status").uppercase()))
                "ReplyHeartbeat" -> PresenceEffect.ReplyHeartbeat(effectObj.getString("pairing_id"))
                else -> error("CallCoreBridge: unknown presence effect kind from native layer: $kind")
            }
        }
        return PresenceUpdateResult(presenceEffects, parseCallEffects(obj.getJSONArray("call_effects").toString()))
    }
}
