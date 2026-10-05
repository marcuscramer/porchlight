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
import com.vitorpamplona.quartz.nip01Core.relay.commands.toClient.Message
import com.vitorpamplona.quartz.nip01Core.relay.commands.toClient.OkMessage
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
 *   `call-core`'s signal router hands these to `CameraAgentService`, which
 *   executes the resulting pairing effects — the actual SPAKE2 state machine
 *   lives in `call-core`, not here; this class only transports them.
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
 * `presence::HEARTBEAT_INTERVAL_MS`, plus one immediately whenever something
 * needs a prompt answer (see [currentHeartbeatIntervalMs]/[kickHeartbeat]); its
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
    private val context: android.content.Context,
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
        /** One relay event, routed by `call-core` (see
         * [CallCoreBridge.routeEvent]). The listener executes, in this order:
         * persist [CallCoreBridge.RouteResult.signal] (immediately, not
         * batched, so a redelivery of that exact event is still rejected even
         * if the app dies right after), apply `update`, run `actions`, then
         * `bootstrapEffects`. A heartbeat answer is already sent by this
         * class and not among the actions. */
        fun onRouted(result: CallCoreBridge.RouteResult)
        /** One [monitor] sweep's worth of [CallCoreBridge.checkCallTimeout]
         * effects — the listener applies these the same way it already
         * applies every other `CallEffect` list. Usually empty; non-empty
         * only when a claimed-but-never-connected call slot just expired. */
        fun onCallTimeoutCheck(effects: List<CallCoreBridge.CallEffect>)
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
        val autoAnswer: Boolean,
    )

    /**
     * A pairing awaiting confirmation. [rendezvousTag] and [bootstrapPayloads]
     * are null/empty when there's no *live* attempt right now (e.g. this device
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
        val bootstrapPayloads: List<JSONObject>,
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

    /**
     * The relays in use: `call-core`'s current list plus the relays a newer list just dropped, which stay for their
     * grace period (see `relay_list`). Read on [executor] and by [snapshot]; replaced by [refreshRelays] and the
     * periodic [monitor] sweep, never edited in place. The built-in list is the fallback if `call-core` has none.
     */
    @Volatile private var relayUrls: Set<NormalizedRelayUrl> = currentRelaySet()

    private fun currentRelaySet(): Set<NormalizedRelayUrl> {
        val urls = CallCoreBridge.relayListCurrent(System.currentTimeMillis()).all.ifEmpty { GeneratedSharedConfig.RELAYS }
        return urls.mapNotNull { it.normalizeRelayUrlOrNull() }.toSet()
    }

    /** The list changed (or a grace period ended): reconnect to the new set. Safe to call from any thread. */
    fun refreshRelays() {
        executor.execute { switchRelaysIfChanged() }
    }

    private fun switchRelaysIfChanged() {
        if (closed) return
        val next = currentRelaySet()
        if (next.isEmpty() || next == relayUrls) return
        Log.i(TAG, "relay list changed: ${relayUrls.size} -> ${next.size} relays")
        relayUrls = next
        rebuildConnections()
    }

    // pingInterval: without it OkHttp never notices a relay socket that died
    // silently (NAT mapping expired, Wi-Fi roam, relay restart with no FIN) —
    // the connection stays "open" while every publish vanishes and nothing
    // arrives, with connectedRelays still counting it as healthy. A ping every
    // 30s fails such a socket within one missed pong, which fires the normal
    // disconnect/reconnect path.
    private fun newHttpClient() = OkHttpClient.Builder().pingInterval(30, TimeUnit.SECONDS).build()

    // Replaced as a pair by [rebuildConnections]; everything but the watchdog uses whatever is current.
    private var httpClient = newHttpClient()
    private val websocketBuilder = object : WebsocketBuilder {
        override fun build(url: NormalizedRelayUrl, out: WebSocketListener): WebSocket =
            BasicOkHttpWebSocket(url, { httpClient }, out)
    }
    private var client = NostrClient(websocketBuilder, scope)

    // Which NostrClient the connection callbacks belong to. A client that [rebuildConnections] threw away can still
    // report its sockets closing; those reports must not touch the state of the new one.
    private var clientGeneration = 0

    // Per-pairing pubkey cache, keyed by ownPrivateKeyHex — cheap to
    // recompute, but every heartbeat tick touches every confirmed pairing's
    // pubkey, so caching avoids rebuilding a KeyPair from raw bytes that
    // often. Never evicted: this app has at most a handful of pairings.
    private val ownPubkeyCache = HashMap<String, String>()

    private fun ownPubkeyHexFor(ownPrivateKeyHex: String): String =
        ownPubkeyCache.getOrPut(ownPrivateKeyHex) { KeyPair(privKey = ownPrivateKeyHex.hexToByteArray()).pubKey!!.toHexKey() }

    // None of these are @Volatile: every write and every read is
    // marshaled onto [executor], so single-writer confinement is what
    // makes them safe.
    //
    // A set of which relays are actually connected right now, not a plain
    // increment/decrement counter — found live, 2026-10-01: a counter only
    // reflects reality if onConnected/onDisconnected fire in matched pairs,
    // but a relay that fails to (re)connect reports through onCannotConnect
    // instead, which a bare counter can't fold in without risking a
    // double-decrement (that callback can also fire for a relay that was
    // never counted as connected at all). A set makes "mark this relay not
    // connected" idempotent no matter which callback said so, so a relay
    // stuck failing to reconnect (ban, outage, anything) is correctly
    // reflected in connectedRelays.size instead of leaving the degraded
    // count silently stale — which had been quietly defeating [monitor]'s
    // own watchdog below.
    private val connectedRelays = mutableSetOf<NormalizedRelayUrl>()

    // What the "Status" screen reads from the UI thread: copies and
    // concurrent maps, written from the callbacks below, so nothing here needs
    // [executor] (unlike [connectedRelays] itself).
    @Volatile private var connectedSnapshot: Set<String> = emptySet()
    @Volatile private var lastHeartbeatSentAtMs: Long? = null
    private var closed = false
    private var heartbeatFuture: java.util.concurrent.ScheduledFuture<*>? = null

    // Retry bookkeeping for publishes that missed a relay lives in call-core
    // (`signal_retry`, see its doc for the full design); this class only
    // supplies the two things that are genuinely native: which relays were
    // connected at send time, and each relay's real NIP-01 OK. quartz-android
    // itself never retries — its PoolEventOutbox records per-relay send
    // state but nothing in the library reads it back out (confirmed by
    // decompiling 1.08.0 and 1.12.6) — so without this a relay that's briefly
    // down when publish() fires loses that message for good.

    data class Snapshot(val relays: List<CallCoreBridge.RelayView>, val lastHeartbeatSentAtMs: Long?)

    /** Safe to call from any thread. */
    fun snapshot(nowMs: Long): Snapshot =
        Snapshot(CallCoreBridge.relayView(relayUrls.map { it.url }, connectedSnapshot, nowMs), lastHeartbeatSentAtMs)

    // The relay cooldowns (see call-core's `signal_retry`) are kept across app
    // restarts, so a relay that rejected us a minute ago isn't poked again by
    // every restart.
    private val cooldownPrefs get() = context.getSharedPreferences("relay_cooldowns", android.content.Context.MODE_PRIVATE)

    /** Keeps the running pauses and, beside them, each relay's last error — a pause without its reason is a mystery after a restart. */
    private fun saveRelayPauses(now: Long) {
        cooldownPrefs.edit().putString("json", CallCoreBridge.exportRelayMemory(now)).apply()
    }

    private fun restoreRelayPauses(now: Long) {
        CallCoreBridge.importRelayMemory(cooldownPrefs.getString("json", "{}") ?: "{}", now)
    }

    fun connect() {
        restoreRelayPauses(System.currentTimeMillis())
        // NostrClient/OkHttp invoke these on their own connection/socket
        // threads, not [executor] — marshaled here so connectedRelays
        // is never touched from more than one thread.
        listenToConnections()
        client.connect()
        resubscribe()

        scheduleHeartbeat(0)
        executor.scheduleWithFixedDelay({ monitor() }, ONLINE_CHECK_INTERVAL_MS, ONLINE_CHECK_INTERVAL_MS, TimeUnit.MILLISECONDS)
    }

    private fun listenToConnections() {
        val generation = clientGeneration
        client.addConnectionListener(object : RelayConnectionListener {
            // safeExecute, not execute: client.disconnect() during
            // close()/teardown fires these from OkHttp's own thread for
            // every relay, which can race callExecutor's own shutdown().
            override fun onConnected(relay: IRelayClient, pingMillis: Int, compressed: Boolean) {
                CallCoreBridge.noteRelayConnected(relay.url.url)
                executor.safeExecute {
                    if (generation != clientGeneration) return@safeExecute
                    val wasEmpty = connectedRelays.isEmpty()
                    connectedRelays.add(relay.url)
                    connectedSnapshot = connectedRelays.mapTo(HashSet()) { it.url }
                    if (wasEmpty && connectedRelays.isNotEmpty()) {
                        listener.onSignalingConnected()
                        // Signaling just came back (or came up for the first
                        // time — the t=0 heartbeat in connect() went out
                        // before any relay was connected): what this device
                        // believes about its peers is stale or empty, and
                        // theirs about this one may be too. Say hello now
                        // rather than waiting out a full heartbeat interval.
                        CallCoreBridge.requestHello()
                        kickHeartbeat()
                    }
                }
            }
            override fun onDisconnected(relay: IRelayClient) {
                executor.safeExecute {
                    if (generation != clientGeneration) return@safeExecute
                    connectedRelays.remove(relay.url)
                    connectedSnapshot = connectedRelays.mapTo(HashSet()) { it.url }
                    if (connectedRelays.isEmpty()) listener.onSignalingDisconnected()
                }
            }
            override fun onCannotConnect(relay: IRelayClient, errorMessage: String) {
                Log.w(TAG, "cannot connect to ${relay.url}: $errorMessage")
                CallCoreBridge.noteRelayConnectError(relay.url.url, errorMessage, System.currentTimeMillis())
                // This relay is, by definition, not connected right now —
                // fold it into the same tracking onDisconnected uses so a
                // relay stuck failing to reconnect doesn't leave
                // connectedRelays (and so monitor()'s degraded check) stale.
                // See this property's own doc for why a set, not a counter.
                executor.safeExecute {
                    if (generation != clientGeneration) return@safeExecute
                    connectedRelays.remove(relay.url)
                    connectedSnapshot = connectedRelays.mapTo(HashSet()) { it.url }
                    if (connectedRelays.isEmpty()) listener.onSignalingDisconnected()
                }
            }
            // The real per-relay, per-event ack the retry queue needs — NOT
            // RelayConnectionListener.onSent, whose "success" is only
            // WebSocket.send() returning true (the local socket accepted the
            // write, nothing about the relay) and whose string param is the
            // command JSON, not an event id (checked by decompiling
            // BasicRelayClient.sendIfConnected). A relay's actual NIP-01
            // response to an EVENT is OkMessage, delivered here because
            // onIncomingMessage fires for every raw message type a relay
            // sends. A relay that was never connected enough to be sent to
            // gets no OK at all — publishAndQueueMisses covers that case.
            override fun onIncomingMessage(relay: IRelayClient, msgStr: String, msg: Message) {
                CallCoreBridge.noteRelayMessage(relay.url.url, System.currentTimeMillis())
                val ok = msg as? OkMessage ?: return
                executor.safeExecute {
                    val now = System.currentTimeMillis()
                    CallCoreBridge.recordPublishResult(ok.eventId, relay.url.url, ok.success, ok.message, now)
                    if (!ok.success) saveRelayPauses(now)
                }
            }
        })
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
                executor.safeExecute { routeIncoming(event) }
            }
            // A relay actively terminating our REQ (NIP-01 CLOSED — rate
            // limiting, auth required, PoW required on the filter, etc.)
            // left the WebSocket connection itself untouched, so neither
            // this class's own reconnect watchdog nor anything else ever
            // noticed: found live, 2026-10-01, by deliberately auditing for
            // more instances of the exact shape that bug took (a listener
            // interface's "something went wrong" callback never wired to
            // any state or recovery). re-subscribing is exactly what's
            // needed (resubscribe()'s own doc: safe to call as often as
            // needed), just debounced so a relay that keeps closing for a
            // persistent reason doesn't get hammered in a tight loop.
            override fun onClosed(message: String, relay: NormalizedRelayUrl, forFilters: List<Filter>?) {
                Log.w(TAG, "relay closed our subscription: $relay: $message")
                executor.safeExecute { resubscribeIfNotRecentlyClosed() }
            }
            override fun onCannotConnect(relay: NormalizedRelayUrl, message: String, forFilters: List<Filter>?) {
                Log.w(TAG, "cannot subscribe on $relay: $message")
            }
        })
    }

    private var lastCloseResubscribeMs = 0L

    private fun resubscribeIfNotRecentlyClosed() {
        val now = System.currentTimeMillis()
        if (now - lastCloseResubscribeMs < SUBSCRIPTION_CLOSE_RESUBSCRIBE_COOLDOWN_MS) return
        lastCloseResubscribeMs = now
        resubscribe()
    }

    /**
     * Everything about an incoming event — dedup, unwrap and verify,
     * parsing, presence, the per-message decisions — is `call-core`'s
     * `signal_router`; this only hands it a fresh snapshot of the pairings,
     * sends the heartbeat answer it asks for, and passes the rest on.
     */
    private fun routeIncoming(event: Event) {
        val confirmed = resolver.confirmedPeers().map {
            CallCoreBridge.RoutePeer(it.pairingId, it.ownPrivateKeyHex, it.peerPublicKey, it.lastSignalCreatedAt, it.lastSignalEventId, it.autoAnswer)
        }
        val pending = resolver.pendingPairings().mapNotNull { p -> p.rendezvousTag?.let { CallCoreBridge.RoutePending(p.pairingId, it) } }
        val result = CallCoreBridge.routeEvent(event.toJson(), resolver.deviceName(), confirmed, pending, System.currentTimeMillis())
        for (action in result.actions) {
            if (action is CallCoreBridge.RouteAction.SendHeartbeat) sendConfirmedOrPending(action.pairingId) { action.payloadJson }
        }
        listener.onRouted(result.copy(actions = result.actions.filterNot { it is CallCoreBridge.RouteAction.SendHeartbeat }))
    }

    private fun heartbeatTick() {
        // isCallActive(): this device's own single call slot, broadcast
        // identically to every contact regardless of who (if anyone) it's
        // actually occupied by. Read once per tick, not once per peer.
        val busy = CallCoreBridge.isCallActive()
        // Built once per tick, not once per peer: whether this heartbeat
        // carries `hello` is consumed by the build itself (see
        // `presence::take_hello`), so one payload has to serve every peer.
        val heartbeat = CallCoreBridge.buildHeartbeatPayload(resolver.deviceName(), busy)
        if (heartbeat != null) {
            lastHeartbeatSentAtMs = System.currentTimeMillis()
            // One event per contact, spread out rather than all in the same
            // instant: relays throttle bursts ("rate-limited: slow down"), and
            // a rejection that arrives before the next send pauses that relay
            // for the rest of them.
            for ((i, peer) in resolver.confirmedPeers().withIndex()) {
                if (i == 0) sendToConfirmedPeer(peer) { heartbeat }
                else executor.safeSchedule(i * GeneratedSharedConfig.HEARTBEAT_SPREAD_MS, TimeUnit.MILLISECONDS) { sendToConfirmedPeer(peer) { heartbeat } }
            }
        }
        for (pending in resolver.pendingPairings()) {
            val tag = pending.rendezvousTag ?: continue
            for (payload in pending.bootstrapPayloads) publishBootstrap(pending.ownPrivateKeyHex, tag, pending.bootstrapTarget, payload)
        }
    }

    /**
     * Slower or faster only for one reason: a live pairing attempt republishes
     * its bootstrap messages on this same tick, so the tick runs more often
     * while one exists (the decision, and the cadence itself, live in
     * `call-core`'s `presence::current_heartbeat_interval_ms`). Everything else
     * that wants a prompt heartbeat — a call placed to an offline-looking peer,
     * regained connectivity, a peer coming online — asks for exactly one via
     * [kickHeartbeat] and a `hello`, instead of raising the rate.
     */
    private fun currentHeartbeatIntervalMs(): Long = CallCoreBridge.currentHeartbeatIntervalMs().toLong()

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

    /**
     * Also the relay-connection watchdog, not just the presence-timeout
     * sweep its name describes — piggybacking on the same 10s tick rather
     * than adding a second timer. What counts as unhealthy, and whether to
     * reconnect softly or rebuild the connections, is `call-core`'s
     * `relay_watchdog`; this only carries it out. quartz-android's own
     * [NostrClient.reconnect] respects `BasicRelayClient`'s per-relay backoff
     * (including a day-long one), so the soft step uses
     * `reconnect(onlyIfChanged = false)`, an unconditional disconnect and
     * connect that clears it.
     */
    private fun monitor() {
        val now = System.currentTimeMillis()
        listener.onPresenceUpdate(CallCoreBridge.checkOnlineTimeouts(now))
        listener.onCallTimeoutCheck(CallCoreBridge.checkCallTimeout(now))
        retryPendingPublishes()
        // A grace period may have ended since the last tick.
        switchRelaysIfChanged()

        when (CallCoreBridge.relayWatchdog(connectedRelays.size, relayUrls.size, now)) {
            CallCoreBridge.RelayWatchdogAction.NOTHING -> {}
            CallCoreBridge.RelayWatchdogAction.SOFT_RECONNECT -> {
                Log.w(TAG, "relays under-connected (${connectedRelays.size}/${relayUrls.size}), reconnecting")
                client.reconnect(onlyIfChanged = false)
            }
            CallCoreBridge.RelayWatchdogAction.REBUILD -> {
                Log.w(TAG, "relays still under-connected (${connectedRelays.size}/${relayUrls.size}) after reconnecting, rebuilding the connections")
                rebuildConnections()
            }
        }
    }

    /**
     * Throws the connection objects and their sockets away and builds new ones. After hours of reconnecting, the
     * connection library can end up with sockets it no longer tracks (64 were open on a Portal that had been
     * deaf for hours) and a connect call that does nothing; the only reliable cure is a fresh [NostrClient] and a
     * fresh [OkHttpClient] whose dispatcher and pool are emptied.
     */
    private fun rebuildConnections() {
        Log.i(TAG, "rebuilding the relay connections")
        val oldClient = client
        val oldHttp = httpClient
        clientGeneration++
        connectedRelays.clear()
        connectedSnapshot = emptySet()
        httpClient = newHttpClient()
        client = NostrClient(websocketBuilder, scope)
        runCatching { oldClient.disconnect() }
        closeSockets(oldHttp)
        listenToConnections()
        client.connect()
        resubscribe()
    }

    private fun closeSockets(http: OkHttpClient) {
        runCatching { http.dispatcher.cancelAll() }
        runCatching { http.connectionPool.evictAll() }
        runCatching { http.dispatcher.executorService.shutdown() }
    }

    // --- Outbound API mirroring the old SignalingClient's shape ------------
    //
    // Purely the wire-send side: plain data in, one publish out, called by
    // CameraAgentService's `applyCallEffects` wherever a `CallEffect` says
    // to send something.

    // Each payload is built by call-core itself (see CallCoreBridge's own
    // `SignalMessage` doc) — the schema lives in one place instead of two
    // independently hand-assembled copies (this class and app.js).
    fun hangUp(pairingId: String, callId: String, mediaFailed: Boolean = false) = sendConfirmedOrPending(pairingId) { CallCoreBridge.buildByePayload(callId, mediaFailed) }
    fun sendBusy(pairingId: String, callId: String) = sendConfirmedOrPending(pairingId) { CallCoreBridge.buildBusyPayload(callId) }
    fun sendCall(pairingId: String, callId: String) = sendConfirmedOrPending(pairingId) { CallCoreBridge.buildCallPayload(callId) }
    fun sendOffer(pairingId: String, sdp: String, callId: String) = sendConfirmedOrPending(pairingId) { CallCoreBridge.buildOfferPayload(sdp, callId) }
    fun sendAnswer(pairingId: String, sdp: String, callId: String) = sendConfirmedOrPending(pairingId) { CallCoreBridge.buildAnswerPayload(sdp, callId) }
    fun sendIce(pairingId: String, sdpMid: String?, sdpMLineIndex: Int, candidate: String, callId: String) =
        sendConfirmedOrPending(pairingId) { CallCoreBridge.buildIcePayload(sdpMid, sdpMLineIndex, candidate, callId) }

    /** One ordinary heartbeat to just [peer], right now — the answer to its
     * `hello`. Deliberately a fresh build: [CallCoreBridge.buildHeartbeatPayload]
     * only sets `hello` once per [CallCoreBridge.requestHello], never here
     * (that would make two devices answer each other forever). */
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
     * `pake-confirm` the moment it's computed. [targetPubkeyHex] is the
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
                publishAndQueueMisses(wrapEvent, wrapJson, payloadJson)
            } catch (t: Throwable) {
                Log.e(TAG, "failed to encrypt/publish to $targetPubkeyHex", t)
            }
        }
    }

    /**
     * The one place a first publish happens: sends to the relays call-core
     * says aren't cooling down from a recent rejection, then hands call-core's
     * retry queue every relay that missed it — cooling down, or not in
     * [connectedRelays] right now (a disconnected relay's socket was never
     * sent anything, so it can't send back an OkMessage either). A connected
     * relay that still rejects the event is recorded separately, by the
     * onIncomingMessage override. Runs on [executor] (via [scope]'s own
     * dispatcher), same as every other read of [connectedRelays].
     */
    private fun publishAndQueueMisses(event: Event, eventJson: String, payloadJson: String) {
        val now = System.currentTimeMillis()
        val available = CallCoreBridge.availableRelays(relayUrls.map { it.url }, now).toSet()
        val targets = relayUrls.filter { it.url in available }.toSet()
        if (targets.isNotEmpty()) client.publish(event, targets)
        val missed = relayUrls.filter { it !in targets || it !in connectedRelays }.map { it.url }
        if (missed.isNotEmpty()) CallCoreBridge.recordPendingPublish(event.id, eventJson, payloadJson, missed, now)
    }

    /** Republishes whatever call-core says is still outstanding — called
     * from [monitor]'s existing 10s tick. Resending to a relay that already
     * has the event is harmless (the receiving side dedupes by event id). */
    private fun retryPendingPublishes() {
        for (retry in CallCoreBridge.dueForRetry(System.currentTimeMillis())) {
            val event = Event.fromJsonOrNull(retry.eventJson) ?: continue
            client.publish(event, retry.relays.mapNotNull { it.normalizeRelayUrlOrNull() }.toSet())
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
                publishAndQueueMisses(event, eventJson, payload.toString())
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
            if (executor.safeSchedule(500, TimeUnit.MILLISECONDS) { client.disconnect(); closeSockets(httpClient) } == null) {
                client.disconnect()
                closeSockets(httpClient)
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
        private const val ONLINE_CHECK_INTERVAL_MS = GeneratedSharedConfig.PRESENCE_TICK_INTERVAL_MS

        // See resubscribe()'s onClosed override: a relay sending NIP-01
        // CLOSED for a persistent reason (PoW/auth it'll never satisfy)
        // would otherwise get re-subscribed in a tight loop forever.
        private const val SUBSCRIPTION_CLOSE_RESUBSCRIBE_COOLDOWN_MS = GeneratedSharedConfig.SUBSCRIPTION_RESUBSCRIBE_COOLDOWN_MS
    }
}
