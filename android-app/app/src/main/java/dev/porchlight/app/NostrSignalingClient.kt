package dev.porchlight.app

import android.util.Log
import com.vitorpamplona.quartz.nip01Core.core.Event
import com.vitorpamplona.quartz.nip01Core.core.hexToByteArray
import com.vitorpamplona.quartz.nip01Core.core.toHexKey
import com.vitorpamplona.quartz.nip01Core.crypto.KeyPair
import com.vitorpamplona.quartz.nip01Core.relay.client.NostrClient
import com.vitorpamplona.quartz.nip01Core.relay.client.listeners.RelayConnectionListener
import com.vitorpamplona.quartz.nip01Core.relay.client.reqs.SubscriptionListener
import com.vitorpamplona.quartz.nip01Core.relay.client.single.IRelayClient
import com.vitorpamplona.quartz.nip01Core.relay.filters.Filter
import com.vitorpamplona.quartz.nip01Core.relay.normalizer.NormalizedRelayUrl
import com.vitorpamplona.quartz.nip01Core.relay.normalizer.normalizeRelayUrlOrNull
import com.vitorpamplona.quartz.nip01Core.relay.sockets.WebSocket
import com.vitorpamplona.quartz.nip01Core.relay.sockets.WebSocketListener
import com.vitorpamplona.quartz.nip01Core.relay.sockets.WebsocketBuilder
import com.vitorpamplona.quartz.nip01Core.relay.sockets.okhttp.BasicOkHttpWebSocket
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import org.json.JSONObject
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit

/** One not-yet-confirmed candidate for a pending pairing — the peer a human
 * taps "Pair with [name]?" for once the SPAKE2 exchange has already
 * cryptographically confirmed it. Unlike the old QR/fingerprint flow,
 * there's normally at most one of these per pending pairing at a time —
 * seeing a second distinct sender aborts the attempt outright rather than
 * presenting a choice (see `call-core`'s pairing-bootstrap collision
 * handling, fronted by [CallCoreBridge]). */
data class CandidatePeer(val publicKey: String, val name: String)

/**
 * Signaling over Nostr relays. One instance for the *whole* service, not one
 * per pairing: unlike Ably's channel-per-pairing model, Nostr has no
 * per-pairing connection at all — a device subscribes once, globally, and
 * this class dispatches each incoming event to the right pairing.
 *
 * **Every pairing has its own, fully independent Nostr keypair**
 * (`Pairing.ownPrivateKeyHex`), generated fresh the moment a pairing
 * attempt starts and reused for that one relationship's entire lifetime —
 * no device-wide identity. This class resolves each pairing's own pubkey on
 * demand ([ownPubkeyHexFor]) and subscribes for traffic addressed to *any*
 * of this device's per-pairing pubkeys at once (see [currentFilters]) —
 * compromising one pairing's key never exposes any other.
 *
 * Two entirely different wire shapes coexist, matching the two phases a
 * pairing goes through:
 *
 * - **Bootstrap** (an in-progress pairing attempt, not yet confirmed): a
 *   plain, unwrapped, unencrypted [SIGNAL_KIND] event, self-signed by the
 *   attempt's own (soon-to-be-permanent) keypair, tagged with a `d` tag
 *   equal to the passphrase-derived rendezvous point. No encryption is
 *   possible yet — neither side knows the other's pubkey until this
 *   exchange itself reveals it — and none is needed: a SPAKE2 blinded
 *   message is safe to publish in the open by construction. See
 *   [Listener.onPairingBootstrapMessage] — the actual SPAKE2 state machine
 *   lives in `CameraAgentService`/`call-core`, not here; this class only
 *   transports and dispatches the raw messages.
 * - **Confirmed** (heartbeat/call/offer/answer/ice/bye/busy): every message
 *   is gift-wrapped (see [publish]'s doc) — the pubkey a relay actually
 *   sees is a fresh random one-time key per message, never either side's
 *   real per-pairing identity, closing the "who's talking to whom" metadata
 *   gap plain NIP-44 signing would still leave. Every call attempt also
 *   carries a `callId`, so a stale message from an abandoned attempt can't
 *   be mistaken for part of a fresh one to the same pairing.
 *
 * Presence is a client-side heartbeat here, not a network primitive: Nostr
 * has no membership list. A confirmed pairing gets a heartbeat every
 * `presence::HEARTBEAT_INTERVAL_MS` normally, faster while something needs
 * quick detection (see [currentHeartbeatIntervalMs]/[kickHeartbeat]); its
 * peer is considered online if one arrived within
 * `presence::ONLINE_TIMEOUT_MS`, checked on [monitor]'s own tick — the
 * decision itself lives entirely in `call-core`'s `presence` module,
 * called through [CallCoreBridge]; this class only owns the tick's own
 * polling cadence ([ONLINE_CHECK_INTERVAL_MS]) and applies whatever
 * [Listener.onPresenceUpdate] gets handed back. An explicit "leaving"
 * message on [close] tells peers immediately rather than making them wait
 * out that timeout.
 */
class NostrSignalingClient(
    private val resolver: PairingResolver,
    private val listener: Listener,
    /**
     * One dedicated, single-threaded executor, owned and lifecycle-managed
     * by [CameraAgentService] and shared with [WebRtcEngine] — every piece
     * of this app's call/pairing state is only ever touched from this one
     * thread. Nothing in here needs its own lock, `@Volatile`, or
     * `Atomic*` as a result — single-writer confinement is what closes the
     * race conditions an earlier audit found in this class.
     */
    private val executor: ScheduledExecutorService,
) {
    interface Listener {
        fun onSignalingConnected()
        fun onSignalingDisconnected()
        /** A presence transition (or several, from one [monitor] sweep) —
         * mirrors `call-core`'s own `presence::PresenceUpdateResult` doc.
         * The listener applies
         * [CallCoreBridge.PresenceUpdateResult.presenceEffects] to its own
         * contact UI state and
         * [CallCoreBridge.PresenceUpdateResult.callEffects] the same way it
         * already applies every other `CallEffect` list. */
        fun onPresenceUpdate(result: CallCoreBridge.PresenceUpdateResult)
        /** A wrap event for [pairingId] was just accepted (routed, verified,
         * decrypted, and newer than — or a different event sharing the same
         * second as — that pairing's own previous
         * [ConfirmedPeer.lastSignalCreatedAt]/[ConfirmedPeer.lastSignalEventId])
         * — the listener must persist [createdAt]/[eventId] as that
         * pairing's new `lastSignalCreatedAt`/`lastSignalEventId`
         * immediately (see [Config.Pairing.lastSignalCreatedAt]'s own doc),
         * not batched with some later save, so a redelivery of this exact
         * event is still rejected even if the app crashes/closes right
         * after this call. */
        fun onSignalProcessed(pairingId: String, createdAt: Long, eventId: String)
        /** [callId] identifies which call attempt is ending — the listener
         * must ignore this if it doesn't match whatever attempt it thinks is
         * currently active, since a "bye" from an already-abandoned attempt
         * can otherwise race a fresh one to the same pairing. */
        fun onPeerHangup(pairingId: String, callId: String)
        /** [callId] identifies which of our own call attempts this busy
         * reply is about — see [onPeerHangup]'s own doc for why: a busy
         * about an attempt this device has already moved on from must not
         * tear down a different, later one. [ownPubkeyHex]/[peerPubkeyHex]:
         * same tie-break inputs [onPresenceUpdate]'s own `markSeen` call
         * takes — see [CallCoreBridge.handlePeerBusyReply]'s own doc for why
         * this needs them too now. */
        fun onPeerBusy(pairingId: String, ownPubkeyHex: String, peerPubkeyHex: String, callId: String)
        fun onPeerNameUpdated(pairingId: String, name: String)
        /**
         * A raw, verified-signature bootstrap message arrived for a pending
         * pairing attempt — [type] is `"pake1"` (the peer's outbound SPAKE2
         * blinded message + self-reported name) or `"pake-confirm"` (the
         * peer's key-confirmation HMAC tag). [senderPubkey] is this
         * message's sender — already signature-verified, but *not yet*
         * trusted as the pairing's real peer. The listener forwards this
         * straight into `CallCoreBridge`/`call-core`, which owns the entire
         * state machine now.
         */
        fun onPairingBootstrapMessage(pairingId: String, senderPubkey: String, type: String, payload: JSONObject)
        /** [callId] is this call attempt's correlation id — the listener
         * must remember it and echo it on every message it sends for this
         * attempt (offer/answer/ice/bye), and reject anything it receives
         * for this pairing tagged with a different one.
         * [ownPubkeyHex]/[peerPubkeyHex]: the pubkey tie-break that decides
         * whether this side should actually offer now lives inside
         * `CallCoreBridge.handleShouldOffer` itself — this method is called
         * unconditionally on every `"call"` message from a confirmed peer,
         * same as every other message type. */
        fun onShouldOffer(pairingId: String, callId: String, ownPubkeyHex: String, peerPubkeyHex: String)
        fun onOffer(pairingId: String, sdp: String, callId: String)
        fun onAnswer(pairingId: String, sdp: String, callId: String)
        fun onRemoteIce(pairingId: String, sdpMid: String?, sdpMLineIndex: Int, candidate: String, callId: String)
    }

    /** Confirmed peer, keyed by pairing. [ownPrivateKeyHex] is *this side's*
     * permanent identity for this one relationship (see the class doc) —
     * needed on every send/receive since there's no single global signer
     * anymore. [lastSignalCreatedAt]/[lastSignalEventId] mirror
     * [Config.Pairing.lastSignalCreatedAt]/[Config.Pairing.lastSignalEventId]
     * — see those fields' own doc. */
    data class ConfirmedPeer(
        val pairingId: String,
        val ownPrivateKeyHex: String,
        val peerPublicKey: String,
        val lastSignalCreatedAt: Long,
        val lastSignalEventId: String,
    )

    /**
     * A pairing awaiting confirmation. [rendezvousTag] and [bootstrapPayload]
     * are null when there's no *live* attempt right now (e.g. this device
     * restarted mid-attempt, or the attempt already timed out/collided) —
     * heartbeatTick() simply skips publishing anything for it in that case.
     * [bootstrapTarget] is the peer's pubkey once discovered from their own
     * `pake1` (null until then) — once known, the (re)published bootstrap
     * message is also tagged directly at them, not just broadcast at the
     * rendezvous point.
     */
    data class PendingPairing(
        val pairingId: String,
        val ownPrivateKeyHex: String,
        val rendezvousTag: String?,
        val bootstrapPayload: JSONObject?,
        val bootstrapTarget: String?,
    )

    /**
     * Everything [NostrSignalingClient] needs to know about current
     * pairing state — provided by [CameraAgentService], which owns
     * [Config]/the in-progress [CallCoreBridge] attempt bookkeeping. Queried fresh on
     * every heartbeat tick and every incoming event, never cached here, the
     * same "always read fresh, never trust a stale snapshot" pattern
     * [Config] itself already uses.
     */
    interface PairingResolver {
        fun confirmedPeers(): List<ConfirmedPeer>
        fun pendingPairings(): List<PendingPairing>
        fun deviceName(): String
    }

    private val scope = CoroutineScope(executor.asCoroutineDispatcher() + SupervisorJob())

    private val relayUrls: Set<NormalizedRelayUrl> =
        RELAYS.mapNotNull { it.normalizeRelayUrlOrNull() }.toSet()

    private val httpClient = OkHttpClient()
    private val websocketBuilder = object : WebsocketBuilder {
        override fun build(url: NormalizedRelayUrl, out: WebSocketListener): WebSocket =
            BasicOkHttpWebSocket(url, { httpClient }, out)
    }
    private val client = NostrClient(websocketBuilder, scope)

    // Per-pairing pubkey cache, keyed by ownPrivateKeyHex — cheap to
    // recompute, but every heartbeat tick touches every confirmed pairing's
    // pubkey, so caching avoids rebuilding a KeyPair from raw bytes that
    // often. Never evicted: this app has at most a handful of pairings.
    private val ownPubkeyCache = HashMap<String, String>()

    private fun ownPubkeyHexFor(ownPrivateKeyHex: String): String =
        ownPubkeyCache.getOrPut(ownPrivateKeyHex) { KeyPair(privKey = ownPrivateKeyHex.hexToByteArray()).pubKey!!.toHexKey() }

    // None of these three are @Volatile: every write and every read is
    // marshaled onto [executor], so single-writer confinement is what
    // makes them safe.
    private var connectedRelayCount = 0
    private var closed = false
    private var heartbeatFuture: java.util.concurrent.ScheduledFuture<*>? = null

    fun connect() {
        // NostrClient/OkHttp invoke these on their own connection/socket
        // threads, not [executor] — marshaled here so connectedRelayCount
        // is never touched from more than one thread.
        client.addConnectionListener(object : RelayConnectionListener {
            // safeExecute, not execute: client.disconnect() during
            // close()/teardown fires these from OkHttp's own thread for
            // every relay, which can race callExecutor's own shutdown().
            override fun onConnected(relay: IRelayClient, pingMillis: Int, compressed: Boolean) {
                executor.safeExecute { connectedRelayCount++; if (connectedRelayCount == 1) listener.onSignalingConnected() }
            }
            override fun onDisconnected(relay: IRelayClient) {
                executor.safeExecute {
                    connectedRelayCount = (connectedRelayCount - 1).coerceAtLeast(0)
                    if (connectedRelayCount == 0) listener.onSignalingDisconnected()
                }
            }
            override fun onCannotConnect(relay: IRelayClient, errorMessage: String) {
                Log.w(TAG, "cannot connect to ${relay.url}: $errorMessage")
            }
        })
        client.connect()
        resubscribe()

        scheduleHeartbeat(0)
        executor.scheduleWithFixedDelay({ monitor() }, ONLINE_CHECK_INTERVAL_MS, ONLINE_CHECK_INTERVAL_MS, TimeUnit.MILLISECONDS)
    }

    /**
     * Builds the current subscription filter set from scratch — a
     * [WRAP_KIND] filter matching *any* of this device's per-pairing
     * pubkeys if there are any, and a [SIGNAL_KIND] filter matching *any*
     * currently-live pairing attempt's rendezvous tag if there are any.
     * Either half is simply omitted when empty.
     */
    private fun currentFilters(): List<Filter> {
        val ownPubkeys = resolver.confirmedPeers().map { ownPubkeyHexFor(it.ownPrivateKeyHex) }
        val rendezvousTags = resolver.pendingPairings().mapNotNull { it.rendezvousTag }
        val relayFilters = CallCoreBridge.buildRelayFilters(ownPubkeys, rendezvousTags)
        val filters = mutableListOf<Filter>()
        relayFilters.wrapFilter?.let { filters += Filter(kinds = listOf(it.kind), tags = mapOf(it.tagName.toString() to it.tagValues)) }
        relayFilters.bootstrapFilter?.let { filters += Filter(kinds = listOf(it.kind), tags = mapOf(it.tagName.toString() to it.tagValues)) }
        return filters
    }

    /**
     * Re-issues the subscription with a freshly-built filter set — a plain
     * REQ with the same subscription id is how Nostr itself defines
     * "replace this subscription's filters," so this is safe to call as
     * often as needed. Called from [kickHeartbeat] — every call site that
     * already calls that is exactly the same set of moments the *filter
     * set* itself needs recomputing too.
     */
    private fun resubscribe() {
        if (closed) return
        client.subscribe(SUB_ID, relayUrls.associateWith { currentFilters() }, object : SubscriptionListener {
            override fun onEvent(event: Event, isLive: Boolean, relay: NormalizedRelayUrl, forFilters: List<Filter>?) {
                // safeExecute: this is NostrClient's own event-delivery
                // thread, which can hand us one more buffered/in-flight
                // event racing teardown.
                executor.safeExecute {
                    if (!CallCoreBridge.markSeenOrIsDuplicate(event.id)) return@safeExecute
                    when (event.kind) {
                        WRAP_KIND -> handleWrapEvent(event)
                        SIGNAL_KIND -> handleBootstrapEvent(event)
                    }
                }
            }
        })
    }

    /**
     * Unwraps a gift-wrapped signal event and dispatches it to a *confirmed*
     * pairing — see [publish]'s doc for the wrap's shape. The wrap's own `p`
     * tag is one of this device's per-pairing pubkeys, which directly (and
     * uniquely — each pairing's own key is freshly generated, never shared)
     * identifies which pairing this is *before* anything is even decrypted.
     * `CallCoreBridge.unwrapWrappedEventForAny` does that routing plus the
     * entire verify-decrypt-verify-decrypt chain (including the
     * pinned-peer identity check) — see its own doc.
     */
    private fun handleWrapEvent(event: Event) {
        val confirmedPeers = resolver.confirmedPeers()
        val candidates = confirmedPeers.map {
            CallCoreBridge.WrapEventCandidate(it.pairingId, it.ownPrivateKeyHex, it.peerPublicKey, it.lastSignalCreatedAt, it.lastSignalEventId)
        }
        val routed = CallCoreBridge.unwrapWrappedEventForAny(event.toJson(), candidates) ?: return
        val pairing = confirmedPeers.find { it.pairingId == routed.pairingId } ?: return
        // See Listener.onSignalProcessed's own doc — persisted before this
        // is even dispatched, so a redelivery of this exact event (or
        // anything older) is rejected by unwrapWrappedEventForAny itself on
        // any subsequent attempt, reload/restart included.
        listener.onSignalProcessed(routed.pairingId, routed.signalCreatedAt, routed.signalEventId)
        // Parsing (type extraction, per-type field shape/length validation)
        // is CallCoreBridge.parseSignalPayload's job — dispatchFromConfirmedPeer
        // still gets called even when this is null: a validly wrapped/
        // decrypted/signature-verified message from a confirmed peer must
        // still register as "this peer is alive" for presence purposes
        // regardless of whether its own content parses.
        dispatchFromConfirmedPeer(pairing, CallCoreBridge.parseSignalPayload(routed.payloadJson))
    }

    /**
     * Dispatches an unwrapped, unencrypted bootstrap-phase event (see the
     * class doc) — matched to a pending pairing purely by its `d` tag
     * (the passphrase-derived rendezvous point), since neither side's real
     * pubkey is known yet. Signature verification is
     * `CallCoreBridge.verifyBootstrapEvent`'s job now (see its own doc) —
     * still required even though bootstrap content is plaintext.
     */
    private fun handleBootstrapEvent(event: Event) {
        val verified = CallCoreBridge.verifyBootstrapEvent(event.toJson()) ?: return
        val pairingId = resolver.pendingPairings().find { it.rendezvousTag == verified.rendezvousTag }?.pairingId ?: return
        val payload = runCatching { JSONObject(verified.payloadJson) }.getOrNull() ?: return
        val type = payload.optString("type").ifBlank { return }
        listener.onPairingBootstrapMessage(pairingId, verified.senderPubkeyHex, type, payload)
    }

    /**
     * [message] is `null` for an unrecognized type or a malformed payload —
     * `markSeen` still runs unconditionally first regardless (a validly
     * wrapped/decrypted/signature-verified message from a confirmed peer
     * means they're alive, independent of whether this device can make
     * sense of what they actually said).
     */
    private fun dispatchFromConfirmedPeer(pairing: ConfirmedPeer, message: CallCoreBridge.SignalMessage?) {
        val pairingId = pairing.pairingId
        val ownPubkeyHex = ownPubkeyHexFor(pairing.ownPrivateKeyHex)
        val peerBusy = (message as? CallCoreBridge.SignalMessage.Heartbeat)?.busy
        listener.onPresenceUpdate(CallCoreBridge.markSeen(pairingId, ownPubkeyHex, pairing.peerPublicKey, System.currentTimeMillis(), peerBusy))
        message ?: return
        when (message) {
            is CallCoreBridge.SignalMessage.Heartbeat -> {
                if (message.name.isNotBlank()) listener.onPeerNameUpdated(pairingId, message.name)
            }
            is CallCoreBridge.SignalMessage.Leaving -> listener.onPresenceUpdate(CallCoreBridge.handleLeavingMessage(pairingId))
            is CallCoreBridge.SignalMessage.Bye -> listener.onPeerHangup(pairingId, message.callId)
            is CallCoreBridge.SignalMessage.Busy -> listener.onPeerBusy(pairingId, ownPubkeyHex, pairing.peerPublicKey, message.callId)
            is CallCoreBridge.SignalMessage.Call -> {
                // .find, not .first: confirmedPeers() re-reads Config fresh
                // every call, so a concurrent removePairing/reconnect
                // between handleWrapEvent's own lookup and this one is a
                // real possibility, not just theoretical — must degrade to
                // a no-op, not throw.
                val peer = resolver.confirmedPeers().find { it.pairingId == pairingId }
                if (peer != null) {
                    listener.onShouldOffer(pairingId, message.callId, ownPubkeyHexFor(peer.ownPrivateKeyHex), peer.peerPublicKey)
                }
            }
            is CallCoreBridge.SignalMessage.Offer -> listener.onOffer(pairingId, message.sdp, message.callId)
            is CallCoreBridge.SignalMessage.Answer -> listener.onAnswer(pairingId, message.sdp, message.callId)
            is CallCoreBridge.SignalMessage.Ice ->
                listener.onRemoteIce(pairingId, message.sdpMid, message.sdpMLineIndex, message.candidate, message.callId)
        }
    }

    private fun heartbeatTick() {
        CallCoreBridge.pruneStalePending(resolver.pendingPairings().map { it.pairingId })
        // isCallActive(): this device's own single call slot, broadcast
        // identically to every contact regardless of who (if anyone) it's
        // actually occupied by. Read once per tick, not once per peer.
        val busy = CallCoreBridge.isCallActive()
        for (peer in resolver.confirmedPeers()) {
            sendToConfirmedPeer(peer) { CallCoreBridge.buildHeartbeatPayload(resolver.deviceName(), busy) }
        }
        for (pending in resolver.pendingPairings()) {
            val tag = pending.rendezvousTag ?: continue
            val payload = pending.bootstrapPayload ?: continue
            publishBootstrap(pending.ownPrivateKeyHex, tag, pending.bootstrapTarget, payload)
        }
    }

    /**
     * Adaptive cadence, not a flat interval: while anything actually needs
     * fast detection — a pairing attempt new enough a human is plausibly
     * still watching it, or a call attempt waiting on an offline peer —
     * heartbeats (and bootstrap re-publishes) go out at `call-core`'s own
     * `presence::FAST_HEARTBEAT_INTERVAL_MS` instead of the normal
     * `presence::HEARTBEAT_INTERVAL_MS` (the decision itself lives in
     * `CallCoreBridge.currentHeartbeatIntervalMs`). Motivated by a real
     * ~28s detection lag seen in testing a freshly-confirmed pairing under
     * an earlier flat-interval design. Applies globally (every pairing
     * gets the faster rate while *any* one needs it), not per-pairing — a
     * deliberate simplification trading a little extra relay traffic for
     * simpler code.
     */
    private fun currentHeartbeatIntervalMs(): Long {
        val now = System.currentTimeMillis()
        val pendingIds = resolver.pendingPairings().map { it.pairingId }
        return CallCoreBridge.currentHeartbeatIntervalMs(pendingIds, now).toLong()
    }

    private fun scheduleHeartbeat(delayMs: Long) {
        if (closed) return
        // executor shut down between the closed check and scheduling —
        // close() is already tearing this client down, nothing to do — see
        // safeSchedule's own doc.
        heartbeatFuture = executor.safeSchedule(delayMs, TimeUnit.MILLISECONDS) {
            if (closed) return@safeSchedule
            heartbeatTick()
            scheduleHeartbeat(currentHeartbeatIntervalMs())
        }
    }

    /**
     * Sends a heartbeat/bootstrap message right now, switches to the fast
     * cadence immediately, and re-subscribes with a freshly-built filter
     * set — called the moment pairing state actually changed (a new
     * pending attempt starting, a confirmation, a reconnect, a removal) so
     * neither the fast heartbeat rate nor the new/removed pairing's own
     * subscription filter waits for whatever's already scheduled to
     * naturally elapse first.
     */
    fun kickHeartbeat() {
        if (closed) return
        resubscribe()
        heartbeatFuture?.cancel(false)
        scheduleHeartbeat(0)
    }

    private fun monitor() {
        listener.onPresenceUpdate(CallCoreBridge.checkOnlineTimeouts(System.currentTimeMillis()))
    }

    // --- Outbound API mirroring the old SignalingClient's shape ------------
    //
    // Purely the wire-send side: plain data in, one publish out, called by
    // CameraAgentService's `applyCallEffects` wherever a `CallEffect` says
    // to send something.

    // Each payload is built by call-core itself (see CallCoreBridge's own
    // `SignalMessage` doc) — the schema lives in one place instead of two
    // independently hand-assembled copies (this class and app.js).
    fun hangUp(pairingId: String, callId: String) = sendConfirmedOrPending(pairingId) { CallCoreBridge.buildByePayload(callId) }
    fun sendBusy(pairingId: String, callId: String) = sendConfirmedOrPending(pairingId) { CallCoreBridge.buildBusyPayload(callId) }
    fun sendCall(pairingId: String, callId: String) = sendConfirmedOrPending(pairingId) { CallCoreBridge.buildCallPayload(callId) }
    fun sendOffer(pairingId: String, sdp: String, callId: String) = sendConfirmedOrPending(pairingId) { CallCoreBridge.buildOfferPayload(sdp, callId) }
    fun sendAnswer(pairingId: String, sdp: String, callId: String) = sendConfirmedOrPending(pairingId) { CallCoreBridge.buildAnswerPayload(sdp, callId) }
    fun sendIce(pairingId: String, sdpMid: String?, sdpMLineIndex: Int, candidate: String, callId: String) =
        sendConfirmedOrPending(pairingId) { CallCoreBridge.buildIcePayload(sdpMid, sdpMLineIndex, candidate, callId) }

    private fun sendConfirmedOrPending(pairingId: String, buildPayload: () -> String?) {
        val peer = resolver.confirmedPeers().find { it.pairingId == pairingId } ?: return
        sendToConfirmedPeer(peer, buildPayload)
    }

    // buildPayload can return null on a panic inside call-core (see e.g.
    // CallCoreBridge.buildOfferPayload's own doc) — a no-op, same as every
    // other "call-core couldn't build/interpret this" case in this class.
    private fun sendToConfirmedPeer(peer: ConfirmedPeer, buildPayload: () -> String?) {
        val payload = buildPayload() ?: return
        publish(peer.ownPrivateKeyHex, peer.peerPublicKey, payload)
    }

    /**
     * Sends one more bootstrap-phase message for an in-progress pairing
     * attempt — used by `CameraAgentService` to publish its own
     * `pake-confirm` the moment it's computed (see
     * [Listener.onPairingBootstrapMessage]). [targetPubkeyHex] is the
     * candidate's pubkey, already known by this point (this is only ever
     * called *after* their `pake1` arrived) — tagging directly at them
     * alongside the rendezvous `d` tag means a relay doesn't have to
     * broadcast this to everyone still watching the (by now
     * one-candidate-only) rendezvous point.
     */
    fun sendPairingBootstrap(ownPrivateKeyHex: String, rendezvousTag: String, targetPubkeyHex: String, build: JSONObject.() -> Unit) {
        publishBootstrap(ownPrivateKeyHex, rendezvousTag, targetPubkeyHex, JSONObject().apply(build))
    }

    /**
     * Builds, gift-wraps, and publishes one signal message for a *confirmed*
     * pairing. Two layers:
     *
     * 1. **Inner event** — the actual payload, NIP-44 encrypted and signed
     *    by this pairing's own permanent identity.
     * 2. **Gift wrap** — that entire signed inner event, JSON-encoded,
     *    NIP-44 re-encrypted under a *fresh, random, one-time keypair*
     *    generated just for this one message, and signed by that random
     *    key. This is what actually gets published: relays (and anyone
     *    watching them) see only a random pubkey they've never seen before
     *    and will never see again, tagged at the recipient — the real
     *    sender is only recoverable by the recipient, who alone can decrypt
     *    the wrap and then the inner event inside it.
     *
     * Deliberately a simplified two-layer wrap, not NIP-59's full
     * rumor/seal/wrap: the spec's extra "unsigned rumor" layer exists so a
     * broken seal can't be replayed as standalone proof of authorship
     * outside its wrap, a threat that doesn't really apply here since every
     * message is already scoped to one specific recipient via NIP-44.
     * Also deliberately *not* NIP-59's registered kind 1059: that's a
     * regular, relay-persisted kind, and this app needs to stay in the
     * ephemeral 20000-29999 range (see [SIGNAL_KIND]'s doc) — [WRAP_KIND]
     * is a second custom ephemeral kind for the same reason.
     *
     * `CallCoreBridge.buildWrappedEvent` does the actual construction/
     * signing (see its own doc) — this function's job is handing the
     * already-built [payloadJson] to call-core and publishing whatever
     * comes back.
     */
    private fun publish(ownPrivateKeyHex: String, targetPubkeyHex: String, payloadJson: String) {
        scope.launch {
            try {
                val wrapJson = CallCoreBridge.buildWrappedEvent(ownPrivateKeyHex, targetPubkeyHex, payloadJson) ?: return@launch
                val wrapEvent = Event.fromJsonOrNull(wrapJson) ?: return@launch
                client.publish(wrapEvent, relayUrls)
            } catch (t: Throwable) {
                Log.e(TAG, "failed to encrypt/publish to $targetPubkeyHex", t)
            }
        }
    }

    /**
     * Publishes one bootstrap-phase event — plain, unencrypted JSON content
     * (see the class doc for why no encryption is possible or needed at
     * this phase), self-signed by the attempt's own keypair, tagged with
     * the rendezvous `d` tag and (once known) the candidate's own `p` tag —
     * `CallCoreBridge.buildBootstrapEvent`'s job now (see its own doc).
     */
    private fun publishBootstrap(ownPrivateKeyHex: String, rendezvousTag: String, targetPubkeyHex: String?, payload: JSONObject) {
        scope.launch {
            try {
                val eventJson = CallCoreBridge.buildBootstrapEvent(ownPrivateKeyHex, rendezvousTag, targetPubkeyHex, payload.toString()) ?: return@launch
                val event = Event.fromJsonOrNull(eventJson) ?: return@launch
                client.publish(event, relayUrls)
            } catch (t: Throwable) {
                Log.e(TAG, "failed to publish bootstrap message at $rendezvousTag", t)
            }
        }
    }

    /**
     * Tells every confirmed peer this device is going away, so they don't
     * have to wait out `presence::ONLINE_TIMEOUT_MS` to notice — the
     * "leaving" analog of Ably's presence.leave. In-progress pairing
     * attempts aren't told anything: the live-window timeout on the other
     * side already handles an abandoned attempt correctly, and a
     * candidate's pubkey may not even be known yet to tell. Doesn't shut
     * down [executor]: that's shared with WebRtcEngine and owned by
     * CameraAgentService, which shuts it down itself once both are torn
     * down.
     *
     * Called from teardown paths that run synchronously on whatever thread
     * called them (typically the Android main thread), not marshaled onto
     * [executor] — so this function posts its own work onto [executor]
     * rather than touching `closed`/`heartbeatFuture` directly.
     */
    fun close() {
        executor.execute {
            closed = true
            heartbeatFuture?.cancel(false)
            for (peer in resolver.confirmedPeers()) sendToConfirmedPeer(peer) { CallCoreBridge.buildLeavingPayload() }
            // Best-effort flush window for the "leaving" publishes above —
            // if they don't make it out in time, peers just fall back to
            // the ordinary heartbeat timeout, an acceptable degraded case.
            //
            // safeSchedule, not schedule: CameraAgentService can shut this
            // executor down right after calling close(), before this inner
            // call runs. If it was rejected, there's no 500ms flush window
            // left to wait out anyway — disconnect immediately instead.
            if (executor.safeSchedule(500, TimeUnit.MILLISECONDS) { client.disconnect() } == null) {
                client.disconnect()
            }
        }
    }

    companion object {
        private const val TAG = "NostrSignaling"
        private const val SUB_ID = "porchlight-signal"

        // One custom, unregistered kind (outside any NIP's reserved range)
        // for every message type this app sends — used two ways (see the
        // class doc). Ephemeral (20000-29999 per NIP-01): relays don't
        // persist any of this after relaying it live.
        //
        // Not `const` — read from CallCoreBridge.protocolConstants (the
        // Rust crate's own real source of truth) rather than a hand-copied
        // literal that could silently drift from app.js's own copy.
        val SIGNAL_KIND = CallCoreBridge.protocolConstants.signalKind

        // The gift-wrap kind (see publish()'s doc) — a second custom,
        // unregistered, ephemeral kind, distinct from SIGNAL_KIND.
        val WRAP_KIND = CallCoreBridge.protocolConstants.wrapKind

        // HEARTBEAT_INTERVAL_MS/FAST_HEARTBEAT_INTERVAL_MS/
        // FAST_HEARTBEAT_WINDOW_MS/ONLINE_TIMEOUT_MS all live in
        // call-core's `presence` module now — this is the one
        // presence-related constant that stays here, purely this class's
        // own polling cadence for calling CallCoreBridge.checkOnlineTimeouts.
        private const val ONLINE_CHECK_INTERVAL_MS = 10_000L

        // Validated via a standalone spike: all three round-trip a message
        // in a few hundred ms.
        val RELAYS = listOf(
            "wss://relay.damus.io",
            "wss://nos.lol",
            "wss://relay.primal.net",
        )
    }
}
