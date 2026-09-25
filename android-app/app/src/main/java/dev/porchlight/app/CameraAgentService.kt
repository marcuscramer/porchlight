package dev.porchlight.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Binder
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.util.Log
import com.vitorpamplona.quartz.nip01Core.core.hexToByteArray
import com.vitorpamplona.quartz.nip01Core.core.toHexKey
import com.vitorpamplona.quartz.nip01Core.crypto.KeyPair
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import org.json.JSONObject
import org.webrtc.EglBase
import org.webrtc.IceCandidate
import org.webrtc.VideoSink

/**
 * Runs the call engine as a foreground service so capture and WebRTC survive
 * when the app is backgrounded, with a persistent notification while active.
 *
 * See `/CALL_STATE.md` at the repo root before changing `onOffer`,
 * `onShouldOffer`, `hangUp`, `requestCall`, `acceptIncomingCall`, or
 * anything else call-arbitration-related — it's the single write-up of the
 * invariants `call-core`'s `call_arbitration` module now enforces. This
 * class is the imperative shell around it, forwarding events in and
 * executing whatever `CallEffect`s come back. [activePairingId] is purely a
 * UI-facing mirror of `call-core`'s own state, kept in sync by
 * [applyCallEffects]/[onPeerConnected] — not the source of truth.
 *
 * A device can have several simultaneous confirmed pairings (contacts).
 * This holds exactly ONE [NostrSignalingClient] for the whole service —
 * Nostr has no per-pairing channel, so a device subscribes once, globally,
 * for "any event tagged with my pubkey," and dispatches by sender pubkey at
 * the application layer (see that class's own doc). [pairings] here is just
 * data (id -> [Pairing] snapshot), not a live connection.
 *
 * [engine] (the single shared [WebRtcEngine]) is arbitrated by
 * [activePairingId] since only one call can be live at a time. Symmetric
 * peers within any one pairing: whoever's pubkey sorts lower always offers
 * (see [NostrSignalingClient]'s tie-break), the other side just waits.
 *
 * **Threading**: [activePairingId]/[activeCallId], [engine]'s `pc`, and
 * [NostrSignalingClient]'s own internal maps are all confined to one
 * dedicated thread — [callExecutor] — rather than protected with locks or
 * `Atomic*`/`@Volatile` fields. This state is genuinely reachable from at
 * least three different threads otherwise: the Android main thread (every
 * UI-triggered call), [NostrSignalingClient]'s own coroutine dispatch, and
 * libwebrtc's own native signaling thread. Every UI-facing method on this
 * class that mutates shared state posts its work onto [callExecutor];
 * [NostrSignalingClient] and [WebRtcEngine] do the same for whatever
 * arrives on their own foreign threads. The full-teardown path
 * ([stopAgent]/[restartAgent]/[onDestroy], via [tearDownOnExecutor]) posts
 * its work onto [callExecutor] too and blocks briefly for it to finish —
 * see that function's own doc for why.
 */
class CameraAgentService : Service(), WebRtcEngine.Listener, NostrSignalingClient.Listener {

    /** One contact, as seen from the UI — mirrors a [Pairing] plus whatever
     * live/transient state only this running service knows about. */
    data class ContactState(
        val id: String,
        val name: String,
        val isPaired: Boolean,
        // This contact's live presence, from this device's own point of
        // view — mirrors the Rust crate's own `presence::PresenceStatus`
        // doc: exactly one of OFFLINE/ONLINE/BUSY, never a combination.
        // Independent of whether WebRTC has actually connected media yet —
        // the gap between this reaching ONLINE and `connected` following it
        // is the "ICE/STUN isn't getting us through" window.
        val status: CallCoreBridge.PresenceStatus = CallCoreBridge.PresenceStatus.OFFLINE,
        // True only once this pairing's PeerConnection reaches CONNECTED —
        // kept distinct from `status` so the waiting screen can tell "not
        // running" apart from "there, but can't establish media."
        val connected: Boolean = false,
        // At most one entry: the peer a SPAKE2 exchange has already
        // cryptographically confirmed for this pending pairing, awaiting
        // the final human "Pair with [name]?" tap. Never holds more than
        // one — a second distinct sender aborts the attempt outright
        // (pairingCollision) instead of listing a choice.
        val pairingCandidates: List<CandidatePeer> = emptyList(),
        // True if a second, distinct sender showed up at this pending
        // pairing's rendezvous point before it resolved — see
        // onPairingBootstrapMessage's doc. Cleared the next time a fresh
        // attempt starts for this same contact.
        val pairingCollision: Boolean = false,
        // True if a pending pairing's live window elapsed with no
        // successful confirmation — see PAKE_LIVE_WINDOW_MS. Cleared the
        // next time a fresh attempt starts for this same contact.
        val pairingTimedOut: Boolean = false,
    )

    /**
     * Non-null exactly while an incoming call for [AgentState.activePairingId]
     * is ringing and hasn't been applied to the peer connection yet.
     * [autoAnswer] is a snapshot of that contact's setting at the moment the
     * call came in (not re-read live), so toggling it mid-ring can't change
     * how a call already in progress of ringing behaves. [secondsRemaining]
     * only means anything when [autoAnswer] is true; it's 0 for a
     * manual-accept call, which just waits indefinitely for a human tap.
     */
    data class IncomingCall(
        val autoAnswer: Boolean,
        val secondsRemaining: Int,
    )

    data class PendingCallOutcome(
        val pairingId: String,
        val reason: CallCoreBridge.CallOutcomeReason,
    )

    data class AgentState(
        val running: Boolean = false,
        val capturing: Boolean = false,
        // Which pairing, if any, currently owns the one live WebRtcEngine
        // call — null means idle (nobody in a call right now).
        val activePairingId: String? = null,
        val incomingCall: IncomingCall? = null,
        // True from the moment an incoming call is accepted (manual tap or
        // auto-answer countdown) until it either connects or ends — lets
        // HomeScreen tell "we accepted, still negotiating" apart from "we
        // placed an outgoing call, still ringing," both of which otherwise
        // look identical (activePairingId set, not yet peerConnectedOk).
        // Found live on real Portal hardware: without this, an accepted
        // auto-answer call rendered CallingScreen's "Calling" text — as if
        // the Portal were the one placing the call — for the whole
        // negotiation window.
        val acceptedIncoming: Boolean = false,
        val contacts: List<ContactState> = emptyList(),
        // Set by a ShowCallOutcome effect, cleared by dismissCallOutcome() —
        // drives HomeScreen's full-screen "why did this call end" prompt.
        val pendingCallOutcome: PendingCallOutcome? = null,
        // Not localized via R.string here — a data class's own default
        // parameter value has no Context to call getString() with, and
        // this default is structurally never shown anyway: updateNotification
        // only ever fires while running is true, but this exact value only
        // ever appears while running is false (see updateState's own
        // `when` below and stopAgent, both of which reach real
        // getString()-backed text through an actual Context instead).
        val statusText: String = "Disconnected",
        // Mirror of WebRtcEngine's own local-track enabled state, published
        // here so CallScreen's Audio/Video toggles can reflect it — reset to
        // true (both) at the same call-end convergence point that clears
        // activePairingId (see onPeerConnected's own doc).
        val audioEnabled: Boolean = true,
        val videoEnabled: Boolean = true,
    )

    private val _state = MutableStateFlow(AgentState())
    val state: StateFlow<AgentState> = _state

    private var engine: WebRtcEngine? = null
    private var signaling: NostrSignalingClient? = null

    // Found live on real (stock, non-Immortal) Portal TV hardware:
    // dismissing the screensaver sometimes lands on the Portal's own stock
    // home screen instead of resuming Porchlight, apparently more likely
    // the longer the device sat idle first. Porchlight isn't registered as
    // the actual system HOME app (see AndroidManifest.xml's own doc on why
    // just LAUNCHER/LEANBACK_LAUNCHER is deliberate), so the normal path
    // back is Android resuming whatever activity task was in front when
    // the dream engaged — which isn't guaranteed to survive a long idle
    // period. Rather than chase exactly why that resume sometimes doesn't
    // happen, this reacts to the dream actually ending and calls the same
    // bringToForeground() already used for the ring/connect cases, so
    // Porchlight reliably reclaims the screen regardless of the reason the
    // automatic resume didn't. Gated on Config.launchOnBoot (see
    // registerScreensaverReceiver's own doc) — this always-reclaim-the-
    // screen behavior is opt-in, same as auto-launching on boot is.
    private var screensaverReceiver: BroadcastReceiver? = null

    // Keyed by Pairing.id — data only now, not a live connection (see class
    // doc). Kept in sync with Config.pairings by every mutator below.
    private val pairings = LinkedHashMap<String, Pairing>()

    // All pairing-bootstrap state now lives entirely inside `call-core`'s
    // own Rust registry, keyed by pairingId — see CallCoreBridge's own doc.
    // CallCoreBridge.pendingSnapshot(pairingId) returns everything
    // NostrSignalingClient's resolver needs in one call, `null` cleanly
    // when there's no live attempt.

    // Backed by the published AgentState itself rather than a separate
    // field, so state.contacts.find { it.id == state.activePairingId } and
    // this are always the same value.
    private var activePairingId: String?
        get() = _state.value.activePairingId
        set(value) = updateState { it.copy(activePairingId = value) }

    // No more activeCallId/pendingOffer/PendingOffer here — call-core's
    // call_arbitration module owns that state now. Every CallEffect that
    // needs a callId carries it directly, and WebRtcEngine tracks its own
    // pc's pairingId/callId for its native-callback closures.

    // The auto-answer countdown's own self-rearming 1-second tick — see
    // tickIncomingCallCountdown. Kept so a call that resolves early
    // (accepted, declined, or the caller hangs up first) can cancel the
    // still-pending next tick rather than leaving it to fire uselessly
    // against a call that has since claimed the slot.
    private var incomingCallTickFuture: ScheduledFuture<*>? = null

    /** Clears whatever incoming-call-in-progress state exists — safe to call
     * even when there is none. Every path that ends a ringing/counting-down
     * call must go through this so a stale countdown tick can never fire
     * against a call that's already resolved one way or another. */
    private fun clearIncomingCall() {
        incomingCallTickFuture?.cancel(false)
        incomingCallTickFuture = null
        if (_state.value.incomingCall != null) updateState { it.copy(incomingCall = null) }
    }

    /**
     * Ticks an auto-answer countdown one second at a time, so
     * [AgentState.incomingCall] can show a live "N seconds" count. The
     * *decision* (stale/continue/accept) is
     * `CallCoreBridge.tickIncomingCallCountdown`'s; this function only owns
     * the actual timer loop, rescheduling itself onto [callExecutor] on
     * every [CallCoreBridge.TickOutcome.Continue].
     */
    private fun scheduleNextCountdownTick(pairingId: String, callId: String) {
        incomingCallTickFuture = callExecutor?.safeSchedule(1, TimeUnit.SECONDS) {
            when (val outcome = CallCoreBridge.tickIncomingCallCountdown(pairingId, callId)) {
                CallCoreBridge.TickOutcome.Stale -> {}
                is CallCoreBridge.TickOutcome.Continue -> {
                    updateState { it.copy(incomingCall = it.incomingCall?.copy(secondsRemaining = outcome.secondsRemaining)) }
                    scheduleNextCountdownTick(pairingId, callId)
                }
                CallCoreBridge.TickOutcome.ShouldAccept -> acceptIncomingCall()
            }
        }
    }

    /**
     * Accepts whatever ring is currently pending (manual Accept tap, or an
     * auto-answer countdown reaching zero) — see
     * `CallCoreBridge.acceptIncomingCall`'s/`AcceptOutcome`'s own docs for
     * the two things this can mean: [CallCoreBridge.AcceptOutcome.ApplyOffer]
     * (the peer's SDP was already in hand — apply it, replaying any ICE
     * candidates buffered while ringing) or
     * [CallCoreBridge.AcceptOutcome.CreateOffer] (this side won the pubkey
     * tie-break on a `"call"` message — nothing negotiated yet). A no-op if
     * there's nothing pending (e.g. a stray UI tap after the call already
     * resolved some other way).
     */
    fun acceptIncomingCall() = callExecutor?.execute {
        when (val result = CallCoreBridge.acceptIncomingCall() ?: return@execute) {
            is CallCoreBridge.AcceptOutcome.ApplyOffer -> {
                clearIncomingCall()
                activePairingId = result.pairingId
                updateState { it.copy(acceptedIncoming = true) }
                engine?.handleRemoteOffer(result.pairingId, result.callId, result.sdp)
                for (ice in result.iceBuffer) engine?.addRemoteIce(ice.sdpMid, ice.sdpMLineIndex, ice.candidate)
            }
            is CallCoreBridge.AcceptOutcome.CreateOffer -> {
                clearIncomingCall()
                activePairingId = result.pairingId
                updateState { it.copy(acceptedIncoming = true) }
                engine?.createOffer(result.pairingId, result.callId)
            }
        }
    } ?: Unit

    /** Per-contact auto-answer toggle (Settings > Contacts) — see
     * [Pairing.autoAnswer]'s doc for why this is per-contact, not global. */
    fun setAutoAnswer(pairingId: String, enabled: Boolean) = callExecutor?.execute {
        val updated = pairings[pairingId]?.copy(autoAnswer = enabled) ?: return@execute
        pairings[pairingId] = updated
        Config.addOrUpdatePairing(this, updated)
    } ?: Unit

    /** The live call's Audio/Video mute toggles (CallScreen's own control
     * row). Not persisted: [AgentState.audioEnabled]/[videoEnabled] reset
     * to true at the same call-end convergence point every other per-call
     * flag does (see onPeerConnected). */
    fun setAudioEnabled(enabled: Boolean) = callExecutor?.execute {
        engine?.setAudioEnabled(enabled)
        updateState { it.copy(audioEnabled = enabled) }
    } ?: Unit
    fun setVideoEnabled(enabled: Boolean) = callExecutor?.execute {
        engine?.setVideoEnabled(enabled)
        updateState { it.copy(videoEnabled = enabled) }
    } ?: Unit

    // One dedicated thread that owns activePairingId/activeCallId, shared
    // with the NostrSignalingClient and WebRtcEngine instances created
    // alongside it — see the class doc's "Threading" section. Created in
    // startAgent(), shut down in stopAgent()/restartAgent()/onDestroy().
    private var callExecutor: ScheduledExecutorService? = null

    private val mainHandler = Handler(Looper.getMainLooper())

    // Self-rearming update-check timer (see scheduleUpdateCheck()); kept so
    // stopAgent()/onDestroy() can cancel the pending tick instead of it
    // firing once more after the service has already torn down.
    private var updateCheckRunnable: Runnable? = null

    private var wakeLock: PowerManager.WakeLock? = null
    private var callWakeLock: PowerManager.WakeLock? = null

    inner class LocalBinder : Binder() {
        val service get() = this@CameraAgentService
    }
    private val binder = LocalBinder()
    override fun onBind(intent: Intent?): IBinder = binder

    val eglContext: EglBase.Context? get() = engine?.eglBase?.eglBaseContext

    // Marshaled like every other entry point that touches WebRtcEngine's
    // internal state — these four are called from Activity/Compose code
    // (main thread) but the sinks they register are also touched from the
    // executor thread. Fire-and-forget is fine: the caller only needs the
    // VideoSink already `init()`'d before calling these, not any
    // synchronous result back.
    fun attachLocalPreview(sink: VideoSink) = callExecutor?.execute { engine?.attachLocalPreview(sink) } ?: Unit
    fun detachLocalPreview() = callExecutor?.execute { engine?.detachLocalPreview() } ?: Unit
    fun attachRemoteView(sink: VideoSink) = callExecutor?.execute { engine?.attachRemoteView(sink) } ?: Unit
    fun detachRemoteView() = callExecutor?.execute { engine?.detachRemoteView() } ?: Unit

    /**
     * Ends whichever pairing currently owns the call slot — whether that's
     * an actual live call, or just a pending attempt that hasn't connected
     * yet (the "press Back to cancel" state on CallingScreen). Also the
     * decline path for an incoming call that hasn't been accepted yet — see
     * CallingScreen/IncomingCallScreen's shared BackHandler. Doesn't stop
     * the agent — every pairing stays registered and reachable for the next
     * call. Distinct from [stopAgent], which tears everything down for
     * good.
     *
     * Entirely `CallCoreBridge.hangUp`'s decision now — see its own doc for
     * why the historical "read activePairingId/activeCallId after
     * closePeer() instead of before, so the bye always carried null" bug is
     * structurally impossible to reintroduce.
     */
    fun hangUp() = callExecutor?.execute {
        applyCallEffects(CallCoreBridge.hangUp())
    } ?: Unit

    /**
     * Starts a call attempt — the *only* way any call ever starts (presence
     * alone never auto-connects anyone; with several contacts there's no
     * non-arbitrary way to guess which one "just became reachable" means
     * you want to talk to). See `CallCoreBridge.requestCall`'s own doc for
     * the tie-break/deferred-call design. No-ops if a *different* contact
     * already owns the slot.
     */
    fun requestCall(pairingId: String) = callExecutor?.execute {
        val pairing = pairings[pairingId] ?: return@execute
        val ownPubkeyHex = KeyPair(privKey = pairing.ownPrivateKeyHex.hexToByteArray()).pubKey!!.toHexKey()
        val peerOnline = CallCoreBridge.isOnline(pairingId)
        val result = CallCoreBridge.requestCall(pairingId, ownPubkeyHex, pairing.peerPublicKey, peerOnline)
        if (result.callId != null) {
            activePairingId = pairingId
            updateState { it.copy(acceptedIncoming = false) }
        }
        applyCallEffects(result.effects)
    } ?: Unit

    /**
     * A human tapped "Pair with [name]?" for a candidate the SPAKE2
     * exchange already cryptographically confirmed (see
     * [onPairingBootstrapMessage]) — the one remaining manual step, a cheap
     * final sanity check rather than a heavy fingerprint ceremony. Looks
     * the candidate up from the currently-published state rather than
     * asking NostrSignalingClient to track it separately — the same data
     * the confirm screen is already showing.
     */
    fun confirmPeer(pairingId: String, publicKeyHex: String) = callExecutor?.execute {
        val candidate = _state.value.contacts.find { it.id == pairingId }
            ?.pairingCandidates?.find { it.publicKey == publicKeyHex } ?: return@execute
        Config.updatePairingPeer(this, pairingId, candidate.publicKey, candidate.name)
        pairings[pairingId] = pairings[pairingId]?.copy(peerPublicKey = candidate.publicKey, peerName = candidate.name)
            ?: return@execute
        // The SPAKE2 exchange was already consumed by finish() inside
        // handleBootstrapMessage by the time a candidate exists to confirm
        // here — this just tells call-core to stop tracking the attempt.
        CallCoreBridge.cancelAttempt(pairingId)
        signaling?.kickHeartbeat()
        updateContact(pairingId) { it.copy(isPaired = true, name = candidate.name, pairingCandidates = emptyList()) }
    } ?: Unit

    /**
     * Starts a passphrase pairing attempt for a brand-new contact — the
     * *only* way a pairing is ever created now (a stale contact is Delete,
     * then this, with a fresh phrase, rather than a "Reconnect": every
     * attempt is then unambiguously new, never corrupting an existing
     * contact's peer info if it's cancelled or fails). [passphrase] is
     * never persisted anywhere — only this attempt's own fresh keypair is,
     * immediately, so it's ready to become the pairing's permanent identity
     * the moment SPAKE2 succeeds.
     *
     * Returns the (persisted, unconfirmed) [Pairing] synchronously, mostly
     * so a caller can navigate to a screen keyed by its id — construction
     * itself is pure, touching no shared state, so this is safe off
     * [callExecutor]. If the service isn't bound yet at all (the very
     * first pairing ever on a fresh device), the actual attempt is deferred
     * until [startAgent] brings [callExecutor] up — see
     * [pendingFirstAttempt].
     */
    fun startPairing(passphrase: String): Pairing {
        val fresh = Pairing(id = UUID.randomUUID().toString(), ownPrivateKeyHex = KeyPair().privKey!!.toHexKey())
        val executor = callExecutor
        if (executor != null) {
            executor.execute {
                Config.addOrUpdatePairing(this, fresh)
                addPairingState(fresh)
                beginPakeAttempt(fresh, passphrase)
            }
        } else {
            Config.addOrUpdatePairing(this, fresh)
            pendingFirstAttempt = fresh.id to passphrase
            CameraAgentService.start(this)
        }
        return fresh
    }

    /**
     * Actually starts the SPAKE2 exchange for [pairing] — calls into
     * `CallCoreBridge`, which trims/NFC-normalizes the raw typed passphrase
     * and derives the rendezvous tag internally (via `pake-bridge`),
     * registers the live attempt in `call-core`'s own registry, and
     * schedules its live-window timeout. Always called on [callExecutor].
     */
    private fun beginPakeAttempt(pairing: Pairing, rawPassphrase: String) {
        val ownPubkeyHex = KeyPair(privKey = pairing.ownPrivateKeyHex.hexToByteArray()).pubKey!!.toHexKey()
        val start = CallCoreBridge.startAttempt(pairing.id, ownPubkeyHex, Config.load(this).deviceName, rawPassphrase)
        updateContact(pairing.id) { it.copy(pairingCandidates = emptyList(), pairingCollision = false, pairingTimedOut = false) }
        signaling?.kickHeartbeat()
        // safeSchedule, not schedule: this fires up to PAKE_LIVE_WINDOW_MS
        // (120s) later — long enough that a rename or any other
        // restartAgent()/stopAgent() in the meantime can easily have shut
        // callExecutor down first, which throws uncaught from a raw
        // schedule() call (see ExecutorExt.kt's doc).
        callExecutor?.safeSchedule(PAKE_LIVE_WINDOW_MS, TimeUnit.MILLISECONDS) {
            // handleTimeout's own generation check (see CallCoreBridge's
            // doc) is what makes this a no-op for a fresh retry or an
            // already-resolved match.
            applyEffects(CallCoreBridge.handleTimeout(pairing.id, start.generation))
        }
    }

    /**
     * Executes whatever [CallCoreBridge.Effect]s a call into `call-core`
     * returned — the entire imperative-shell half of the pairing-bootstrap
     * state machine now lives in this one function. See each `Effect`
     * variant's own doc (mirrored on the Rust side) for what it means.
     */
    private fun applyEffects(effects: List<CallCoreBridge.Effect>) {
        for (effect in effects) {
            when (effect) {
                is CallCoreBridge.Effect.SendBootstrap -> {
                    val pairing = pairings[effect.pairingId] ?: continue
                    signaling?.sendPairingBootstrap(pairing.ownPrivateKeyHex, effect.rendezvousTag, effect.targetPubkey) {
                        effect.payload.keys().forEach { key -> put(key, effect.payload.get(key)) }
                    }
                }
                CallCoreBridge.Effect.KickHeartbeat -> signaling?.kickHeartbeat()
                is CallCoreBridge.Effect.SetCollision ->
                    updateContact(effect.pairingId) { it.copy(pairingCandidates = emptyList(), pairingCollision = true) }
                is CallCoreBridge.Effect.SetTimedOut ->
                    updateContact(effect.pairingId) { it.copy(pairingTimedOut = true) }
                is CallCoreBridge.Effect.SetConfirmedCandidate -> {
                    updateContact(effect.pairingId) { it.copy(pairingCandidates = listOf(CandidatePeer(effect.pubkeyHex, effect.name))) }
                }
            }
        }
    }

    /**
     * Executes whatever [CallCoreBridge.CallEffect]s a call into `call-core`
     * returned — mirrors [applyEffects] above (pairing-bootstrap's own
     * twin). See each `CallEffect` variant's own doc for what it means.
     *
     * [activePairingId] is set directly here for every effect that means
     * "the call slot is now claimed for this pairing" (`CreateOffer`/
     * `SendCall`/`ApplyRemoteOffer`/`StartRinging`) — matches
     * `CALL_STATE.md` invariant #1: claimed the moment a call *could*
     * happen, not once it's accepted. It's cleared the other way, in
     * [onPeerConnected], not here.
     */
    private fun applyCallEffects(effects: List<CallCoreBridge.CallEffect>) {
        for (effect in effects) {
            when (effect) {
                CallCoreBridge.CallEffect.AcquireMedia -> engine?.acquireMedia()
                is CallCoreBridge.CallEffect.CreateOffer -> {
                    activePairingId = effect.pairingId
                    engine?.createOffer(effect.pairingId, effect.callId)
                }
                is CallCoreBridge.CallEffect.SendCall -> {
                    activePairingId = effect.pairingId
                    signaling?.sendCall(effect.pairingId, effect.callId)
                }
                is CallCoreBridge.CallEffect.SendBusy -> signaling?.sendBusy(effect.pairingId, effect.callId)
                is CallCoreBridge.CallEffect.ApplyRemoteOffer -> {
                    activePairingId = effect.pairingId
                    engine?.handleRemoteOffer(effect.pairingId, effect.callId, effect.sdp)
                }
                is CallCoreBridge.CallEffect.StartRinging -> {
                    activePairingId = effect.pairingId
                    // Ringing needs the wake lock + foreground bring-up as
                    // much as an already-answered call does — see
                    // bringToForeground's own doc.
                    acquireCallWakeLock()
                    bringToForeground()
                    updateState { it.copy(incomingCall = IncomingCall(autoAnswer = effect.autoAnswer, secondsRemaining = effect.secondsRemaining)) }
                    if (effect.autoAnswer) scheduleNextCountdownTick(effect.pairingId, effect.callId)
                }
                is CallCoreBridge.CallEffect.SendBye -> signaling?.hangUp(effect.pairingId, effect.callId)
                CallCoreBridge.CallEffect.ClosePeerConnection -> engine?.closePeer()
                CallCoreBridge.CallEffect.ClearIncomingCallTimer -> clearIncomingCall()
                is CallCoreBridge.CallEffect.ShowCallOutcome ->
                    updateState { it.copy(pendingCallOutcome = PendingCallOutcome(effect.pairingId, effect.reason)) }
            }
        }
    }

    /** Dismisses the full-screen call-outcome prompt — Back or "Call again" tap. */
    fun dismissCallOutcome() = callExecutor?.execute {
        updateState { it.copy(pendingCallOutcome = null) }
    } ?: Unit

    /**
     * "Forget this contact" — deletes the pairing entirely. Not a security
     * remedy; leaves this device's own identity and every *other* pairing
     * untouched.
     */
    fun removePairing(pairingId: String) = callExecutor?.execute {
        pairings.remove(pairingId)
        CallCoreBridge.cancelAttempt(pairingId)
        // See CallCoreBridge.forgetPairing's own doc: clears a deferred
        // call's wants_call entry and, if this pairing owned the active
        // call slot, its ClosePeerConnection effect below tears down the
        // real pc (cascading into onPeerConnected(pairingId, false), which
        // clears activePairingId itself) *without* a bye.
        applyCallEffects(CallCoreBridge.forgetPairing(pairingId))
        CallCoreBridge.presenceRemovePairing(pairingId)
        Config.removePairing(this, pairingId)
        signaling?.kickHeartbeat()
        updateState { it.copy(contacts = it.contacts.filterNot { c -> c.id == pairingId }) }
    } ?: Unit

    /**
     * Tears down and restarts signaling (plus the shared engine) against
     * whatever's currently saved in Config — used any time something
     * Config-derived needs to reach the wire immediately rather than only
     * on next app launch (a device rename, for example).
     */
    fun restartAgent() {
        tearDownOnExecutor(clearPairings = true)
        _state.value = AgentState(running = true)
        startAgent()
    }

    /**
     * Runs the state-mutating part of a full teardown ([clearIncomingCall],
     * [CallCoreBridge.hangUp], closing [signaling], cancelling every pending
     * pairing attempt, stopping [engine]) as one task on [callExecutor], and
     * blocks the caller (always the main thread in practice) until it
     * finishes, rather than running that same sequence directly on the
     * caller's thread — restores the single-writer invariant the rest of
     * this class relies on (see the class doc's "Threading" section);
     * [restartAgent] fires on every device rename, an ordinary user action,
     * so a heartbeat/offer dispatch already running on `callExecutor` could
     * otherwise race this function's own direct mutation of shared state.
     *
     * A short timeout, not an unbounded wait — teardown must still complete
     * even if the executor task unexpectedly hangs; the shutdown below
     * still runs either way.
     */
    private fun tearDownOnExecutor(clearPairings: Boolean) {
        val executor = callExecutor
        if (executor != null) {
            try {
                executor.submit {
                    clearIncomingCall()
                    // Resets call-core's own pending_offer/wants_call/active-ids
                    // state too (see CallCoreBridge.hangUp's doc) — effects
                    // discarded, there's no signaling client left standing to
                    // send them through by the time this runs anyway. Safe even
                    // when nothing was active.
                    CallCoreBridge.hangUp()
                    signaling?.close(); signaling = null
                    pairings.keys.toList().forEach(CallCoreBridge::cancelAttempt)
                    if (clearPairings) pairings.clear()
                    engine?.stop(); engine = null
                }.get(2, TimeUnit.SECONDS)
            } catch (e: Exception) {
                // TimeoutException/ExecutionException/InterruptedException — a
                // stuck or failing teardown task must not block the main
                // thread forever; fall through and shut the executor down
                // regardless.
                Log.w(TAG, "tearDownOnExecutor: teardown task didn't finish cleanly", e)
            }
        }
        callExecutor?.shutdown(); callExecutor = null
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> { stopAgent(); return START_NOT_STICKY }
            else -> startAgent()
        }
        return START_STICKY
    }

    private fun startAgent() {
        if (engine != null) return
        val config = Config.load(this)
        if (!config.isValid) {
            Log.w(TAG, "config invalid; not starting")
            stopSelf()
            return
        }
        startForeground(NOTIF_ID, buildNotification(getString(R.string.notifications_connectingStatus)))
        acquireWakeLock()
        registerScreensaverReceiver()

        val executor = Executors.newSingleThreadScheduledExecutor()
        callExecutor = executor
        engine = WebRtcEngine(context = this, listener = this, executor = executor).also { it.start() }
        signaling = NostrSignalingClient(resolver = PairingResolverImpl(), listener = this, executor = executor).also { it.connect() }
        for (pairing in config.pairings) addPairingState(pairing)

        // See startPairing()/pendingFirstAttempt's doc — the very first
        // pairing ever on a fresh device can be requested before this
        // service (and thus callExecutor) exists at all.
        pendingFirstAttempt?.let { (id, passphrase) ->
            pendingFirstAttempt = null
            config.pairings.find { it.id == id }?.let { pairing -> executor.execute { beginPakeAttempt(pairing, passphrase) } }
        }

        _state.value = _state.value.copy(running = true, statusText = getString(R.string.notifications_connectingStatus))

        scheduleUpdateCheck()
    }

    // Deliberately not on callExecutor: UpdateChecker.checkAndMaybeNotify
    // runs entirely on OkHttp's own background dispatcher and never
    // touches call/pairing state. mainHandler is used purely as a timer —
    // repostDelayed re-arms itself only while the service is still alive.
    private fun scheduleUpdateCheck() {
        UpdateChecker.checkAndMaybeNotify(this)
        val runnable = Runnable { if (engine != null) scheduleUpdateCheck() }
        updateCheckRunnable = runnable
        mainHandler.postDelayed(runnable, UPDATE_CHECK_INTERVAL_MS)
    }

    private fun addPairingState(pairing: Pairing) {
        pairings[pairing.id] = pairing
        // status seeded from call-core's own presence state, not left at
        // ContactState's OFFLINE default: the `presence` module's state is
        // a persistent, process-global Rust static — it survives
        // restartAgent() rebuilding this list from scratch. isPeerBusy
        // checked first: it can only be true when isOnline is also true, so
        // this is a lossless reconstruction of the one status Rust holds.
        val status = when {
            CallCoreBridge.isPeerBusy(pairing.id) -> CallCoreBridge.PresenceStatus.BUSY
            CallCoreBridge.isOnline(pairing.id) -> CallCoreBridge.PresenceStatus.ONLINE
            else -> CallCoreBridge.PresenceStatus.OFFLINE
        }
        val fresh = ContactState(
            id = pairing.id,
            name = pairing.peerName,
            isPaired = pairing.isConfirmed,
            status = status,
        )
        updateState { state ->
            if (state.contacts.any { it.id == pairing.id }) {
                state.copy(contacts = state.contacts.map { if (it.id == pairing.id) fresh else it })
            } else {
                state.copy(contacts = state.contacts + fresh)
            }
        }
    }

    private fun stopAgent() {
        updateCheckRunnable?.let { mainHandler.removeCallbacks(it) }; updateCheckRunnable = null
        tearDownOnExecutor(clearPairings = true)
        releaseWakeLock()
        releaseCallWakeLock()
        unregisterScreensaverReceiver()
        _state.value = AgentState(statusText = getString(R.string.notifications_disconnectedStatus))
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        updateCheckRunnable?.let { mainHandler.removeCallbacks(it) }; updateCheckRunnable = null
        tearDownOnExecutor(clearPairings = false)
        releaseWakeLock()
        releaseCallWakeLock()
        unregisterScreensaverReceiver()
        super.onDestroy()
    }

    /**
     * Keep the CPU running while connected so capture + WebRTC keep streaming
     * even if the screen turns off. Released on disconnect/destroy.
     */
    private fun acquireWakeLock() {
        if (wakeLock?.isHeld == true) return
        val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
        wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "$TAG:call").apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    private fun releaseWakeLock() {
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
    }

    /**
     * Holds the screen on for real during an active call — scoped narrowly to
     * "a peer is actually connected", not the whole running/waiting period,
     * since FULL_WAKE_LOCK forces max brightness. FLAG_KEEP_SCREEN_ON (set in
     * MainActivity) doesn't stop this build's screensaver: Immortal's custom
     * presence-based PowerManager ignores the standard dream settings
     * entirely, and FULL_WAKE_LOCK — deprecated everywhere else — is the
     * working wake path on API 28/29 Portals. Worth the deprecation warning.
     */
    private fun acquireCallWakeLock() {
        if (callWakeLock?.isHeld == true) return
        val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
        @Suppress("DEPRECATION")
        callWakeLock = pm.newWakeLock(
            PowerManager.FULL_WAKE_LOCK or PowerManager.ACQUIRE_CAUSES_WAKEUP or PowerManager.ON_AFTER_RELEASE,
            "$TAG:call-active",
        ).apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    private fun releaseCallWakeLock() {
        callWakeLock?.let { if (it.isHeld) it.release() }
        callWakeLock = null
    }

    // Signaling callbacks can fire from NostrSignalingClient's own
    // coroutine/executor threads, not just this service's main thread — a
    // plain `_state.value = transform(_state.value)` here would be a lost-
    // update race. MutableStateFlow.update does an atomic compare-and-retry
    // loop instead, so no update is ever lost this way.
    private fun updateState(transform: (AgentState) -> AgentState) {
        var next = _state.value
        _state.update { current ->
            val s = transform(current)
            val text = when {
                !s.running -> getString(R.string.notifications_disconnectedStatus)
                s.contacts.any { it.connected } -> getString(R.string.notifications_inCallStatus)
                s.contacts.any { it.status != CallCoreBridge.PresenceStatus.OFFLINE } -> getString(R.string.notifications_waitingForCallStatus)
                else -> getString(R.string.notifications_waitingForContactStatus)
            }
            next = s.copy(statusText = text)
            next
        }
        if (next.running) updateNotification(next.statusText)
    }

    private fun updateContact(pairingId: String, transform: (ContactState) -> ContactState) {
        updateState { state -> state.copy(contacts = state.contacts.map { if (it.id == pairingId) transform(it) else it }) }
    }

    // --- NostrSignalingClient.PairingResolver -------------------------------

    private inner class PairingResolverImpl : NostrSignalingClient.PairingResolver {
        override fun confirmedPeers(): List<NostrSignalingClient.ConfirmedPeer> =
            Config.load(this@CameraAgentService).pairings
                .filter { it.isConfirmed }
                .map { NostrSignalingClient.ConfirmedPeer(it.id, it.ownPrivateKeyHex, it.peerPublicKey, it.lastSignalCreatedAt, it.lastSignalEventId) }

        override fun pendingPairings(): List<NostrSignalingClient.PendingPairing> =
            Config.load(this@CameraAgentService).pairings
                .filterNot { it.isConfirmed }
                .map { pairing ->
                    val snapshot = CallCoreBridge.pendingSnapshot(pairing.id)
                    NostrSignalingClient.PendingPairing(
                        pairingId = pairing.id,
                        ownPrivateKeyHex = pairing.ownPrivateKeyHex,
                        rendezvousTag = snapshot?.rendezvousTag,
                        bootstrapPayload = snapshot?.payload,
                        bootstrapTarget = snapshot?.candidatePubkey,
                    )
                }

        override fun deviceName(): String = Config.load(this@CameraAgentService).deviceName
    }

    // --- NostrSignalingClient.Listener --------------------------------------

    // Required overrides, but nothing in this app currently needs to react
    // to relay-connection transitions specifically (as opposed to a real
    // signal actually being processed, which onSignalProcessed below does
    // drive UI from).
    override fun onSignalingConnected() {}

    override fun onSignalingDisconnected() {}

    override fun onSignalProcessed(pairingId: String, createdAt: Long, eventId: String) {
        Config.updateLastSignal(this, pairingId, createdAt, eventId)
    }

    /**
     * A presence transition (or several, from one timeout sweep) — `presence`
     * calls directly into `call_arbitration` itself, so
     * [CallCoreBridge.PresenceUpdateResult.callEffects] already includes
     * whatever a deferred call resolving or an active call ending as a
     * result of this transition requires. `connected` is additionally
     * cleared on `PresenceStatus.OFFLINE`, matching `ContactState` — web's
     * own `connected` field has no equivalent to clear, an existing
     * platform asymmetry this doesn't fix.
     */
    override fun onPresenceUpdate(result: CallCoreBridge.PresenceUpdateResult) {
        for (effect in result.presenceEffects) {
            when (effect) {
                is CallCoreBridge.PresenceEffect.SetStatus -> updateContact(effect.pairingId) {
                    it.copy(status = effect.status, connected = if (effect.status == CallCoreBridge.PresenceStatus.OFFLINE) false else it.connected)
                }
            }
        }
        applyCallEffects(result.callEffects)
    }

    override fun onPeerHangup(pairingId: String, callId: String) {
        // Gated on callId, not just pairingId — see CallCoreBridge.
        // handle_peer_hangup's own doc: a "bye" from an attempt this device
        // has already moved on from must not tear down a call that isn't
        // the one it's actually about. Peer is still there, just not in a
        // call anymore — leave `online` alone. Also the caller-cancelled-
        // before-we-answered path: a "bye" arriving while still ringing/
        // counting down must cancel that too.
        applyCallEffects(CallCoreBridge.handlePeerHangup(pairingId, callId))
    }

    /**
     * Mirrors receiving a `"busy"` reply to our own outgoing call attempt —
     * see [CallCoreBridge.handlePeerBusyReply]'s own doc. The busy badge is
     * a live presence status now (see [onPresenceUpdate]'s own `SetStatus`
     * handling), continuously correct for as long as the peer's own
     * heartbeats keep reporting it. This call still matters for its own
     * sake: immediate feedback (don't wait for the peer's next heartbeat)
     * plus releasing our own slot.
     */
    override fun onPeerBusy(pairingId: String, ownPubkeyHex: String, peerPubkeyHex: String, callId: String) {
        onPresenceUpdate(CallCoreBridge.handlePeerBusyReply(pairingId, ownPubkeyHex, peerPubkeyHex, callId))
    }

    override fun onPeerNameUpdated(pairingId: String, name: String) {
        // The key that matched to get here is still whatever's already
        // pinned for this pairing — only the display label changed.
        val currentKey = Config.load(this).pairings.find { it.id == pairingId }?.peerPublicKey
        if (currentKey.isNullOrBlank()) return
        Config.updatePairingPeer(this, pairingId, currentKey, name)
        updateContact(pairingId) { it.copy(name = name) }
    }

    /**
     * Drives the entire SPAKE2 pairing state machine for one attempt — now
     * entirely `call-core`'s decision to make. [NostrSignalingClient] only
     * transports and verifies the signature of these messages; this
     * function's entire job is forwarding the event in and executing
     * whatever [CallCoreBridge.Effect]s come back.
     */
    override fun onPairingBootstrapMessage(pairingId: String, senderPubkey: String, type: String, payload: JSONObject) {
        applyEffects(CallCoreBridge.handleBootstrapMessage(pairingId, senderPubkey, type, payload))
    }

    /**
     * See `CallCoreBridge.handleShouldOffer`'s own doc for the guard
     * sequence (pubkey tie-break first, then busy, then redelivery) — this
     * function's whole job is forwarding the event in and executing
     * whatever `CallEffect`s come back, same shape as
     * [onPairingBootstrapMessage] above.
     */
    override fun onShouldOffer(pairingId: String, callId: String, ownPubkeyHex: String, peerPubkeyHex: String) {
        val autoAnswer = pairings[pairingId]?.autoAnswer == true
        applyCallEffects(CallCoreBridge.handleShouldOffer(pairingId, callId, ownPubkeyHex, peerPubkeyHex, autoAnswer))
    }

    /**
     * See `CallCoreBridge.handleOffer`'s own doc for the full guard
     * sequence. This function's own job is just looking up
     * [Pairing.autoAnswer] (a snapshot taken now, not re-read later — see
     * [IncomingCall]'s doc; `call-core` doesn't own contacts) and
     * forwarding the rest.
     */
    override fun onOffer(pairingId: String, sdp: String, callId: String) {
        val autoAnswer = pairings[pairingId]?.autoAnswer == true
        applyCallEffects(CallCoreBridge.handleOffer(pairingId, callId, sdp, autoAnswer))
    }

    override fun onAnswer(pairingId: String, sdp: String, callId: String) {
        if (CallCoreBridge.shouldApplyAnswer(pairingId, callId)) engine?.handleRemoteAnswer(sdp)
    }

    override fun onRemoteIce(pairingId: String, sdpMid: String?, sdpMLineIndex: Int, candidate: String, callId: String) {
        when (val outcome = CallCoreBridge.handleRemoteIce(pairingId, callId, sdpMid, sdpMLineIndex, candidate)) {
            is CallCoreBridge.IceOutcome.Apply -> engine?.addRemoteIce(outcome.sdpMid, outcome.sdpMLineIndex, outcome.candidate)
            CallCoreBridge.IceOutcome.Buffered, CallCoreBridge.IceOutcome.Dropped -> {}
        }
    }

    // --- WebRtcEngine.Listener (shared across every pairing) -----------------
    //
    // pairingId/callId arrive as explicit parameters — captured by
    // WebRtcEngine at negotiation-start time, not read back from this
    // class's own state.

    override fun onLocalOffer(pairingId: String, callId: String, sdp: String) {
        signaling?.sendOffer(pairingId, sdp, callId)
    }

    override fun onLocalAnswer(pairingId: String, callId: String, sdp: String) {
        signaling?.sendAnswer(pairingId, sdp, callId)
    }

    override fun onLocalIce(pairingId: String, callId: String, candidate: IceCandidate) {
        signaling?.sendIce(pairingId, candidate.sdpMid, candidate.sdpMLineIndex, candidate.sdp, callId)
    }

    /**
     * The one place [activePairingId] gets *cleared* (set, on the other
     * hand, happens all over [applyCallEffects]) — `WebRtcEngine.
     * cleanupPeerConnection()` calls this unconditionally on every real
     * `pc` teardown, for *any* reason (an explicit `ClosePeerConnection`
     * effect, WebRTC's own spontaneous FAILED/CLOSED transition, or
     * `onMediaFailure` closing an active call) — the one guaranteed place
     * to react to all of them uniformly.
     */
    override fun onPeerConnected(pairingId: String?, callId: String?, connected: Boolean) {
        if (pairingId != null) updateContact(pairingId) { it.copy(connected = connected) }
        if (connected) {
            if (pairingId != null && callId != null) CallCoreBridge.markConnected(pairingId, callId)
            acquireCallWakeLock()
            bringToForeground()
        } else {
            applyCallEffects(CallCoreBridge.peerConnectionClosed())
            activePairingId = null
            updateState { it.copy(audioEnabled = true, videoEnabled = true, acceptedIncoming = false) }
            releaseCallWakeLock()
            // A peer ending a call needs its next heartbeat kicked out
            // immediately, not left to the slow idle cadence — being on an
            // active call isn't one of current_heartbeat_interval_ms's own
            // fast-cadence conditions, so nothing else speeds this up.
            signaling?.kickHeartbeat()
        }
    }

    /**
     * A call can connect while MainActivity isn't in front — e.g. sitting on
     * Immortal's photo frame — since negotiation happens here in the
     * service regardless of what's on screen. The call-active wake lock
     * already keeps the screen on in that case, but without this, it'd just
     * keep showing whatever was already there while a live call runs
     * silently underneath. Standard "incoming call" pattern: an Activity
     * can't bring itself forward from a background Service any other way.
     */
    private fun bringToForeground() {
        val intent = Intent(this, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        startActivity(intent)
    }

    /**
     * See [screensaverReceiver]'s own doc for why this exists. Gated on
     * [Config.launchOnBoot], same as the user's decision when they choose
     * to name it: someone who wants Porchlight to always come back up on
     * its own (boot included) wants this too, and someone who left that
     * off would find Porchlight unconditionally stealing the screen back
     * from whatever they dismissed the screensaver to use instead just as
     * unwelcome as an uninvited auto-launch on boot.
     */
    private fun registerScreensaverReceiver() {
        if (screensaverReceiver != null) return
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                if (Config.load(context).launchOnBoot) bringToForeground()
            }
        }
        val filter = IntentFilter(Intent.ACTION_DREAMING_STOPPED)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            registerReceiver(receiver, filter)
        }
        screensaverReceiver = receiver
    }

    private fun unregisterScreensaverReceiver() {
        screensaverReceiver?.let { runCatching { unregisterReceiver(it) } }
        screensaverReceiver = null
    }

    override fun onCapturingChanged(capturing: Boolean) {
        updateState { it.copy(capturing = capturing) }
    }

    // --- Notification --------------------------------------------------------

    private fun buildNotification(text: String): Notification {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val mgr = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            if (mgr.getNotificationChannel(CHANNEL_ID) == null) {
                mgr.createNotificationChannel(
                    NotificationChannel(CHANNEL_ID, getString(R.string.notifications_callStatusChannelName), NotificationManager.IMPORTANCE_LOW)
                        .apply { description = getString(R.string.notifications_callStatusChannelDesc) },
                )
            }
        }
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O)
            Notification.Builder(this, CHANNEL_ID) else @Suppress("DEPRECATION") Notification.Builder(this)
        return builder
            .setContentTitle(getString(R.string.notifications_connectedTitle))
            .setContentText(text)
            .setSmallIcon(android.R.drawable.presence_video_online)
            .setOngoing(true)
            .build()
    }

    private fun updateNotification(text: String) {
        val mgr = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        mgr.notify(NOTIF_ID, buildNotification(text))
    }

    companion object {
        private const val TAG = "CameraAgent"
        private const val CHANNEL_ID = "camera_agent"
        private const val NOTIF_ID = 1
        const val ACTION_STOP = "dev.porchlight.app.STOP"

        /**
         * Carries a not-yet-started pairing attempt's passphrase across the
         * Activity/Service boundary for exactly one case: the very first
         * pairing ever on a fresh device, requested before this service
         * exists (or is bound) at all — [startAgent] refuses to even keep
         * running with zero pairings in [Config], so the Pairing row must
         * already be persisted *before* [start] is called, at which point
         * there's no live instance yet to hand the passphrase to any other
         * way (it's never itself persisted — see `CallCoreBridge`'s doc).
         * [MainActivity] sets it directly when its own `service` reference
         * is still null; whichever [CameraAgentService] instance
         * [startAgent] next brings up consumes it.
         */
        private var pendingFirstAttempt: Pair<String, String>? = null

        // "At least 120 seconds" — long enough for two people to type a few
        // words over a call on a TV remote/on-screen keyboard without
        // feeling rushed, short enough not to sit exposed indefinitely. A
        // live pairing attempt that hasn't resolved (crypto-confirmed or
        // collided) by then times out. Read from
        // CallCoreBridge.protocolConstants, not a hand-copied literal.
        private val PAKE_LIVE_WINDOW_MS = CallCoreBridge.protocolConstants.pakeLiveWindowMs

        // AUTO_ANSWER_COUNTDOWN_SECONDS moved into call-core (see
        // CallCoreBridge.handleOffer's StartRinging effect, which carries
        // its own secondsRemaining) — this class no longer picks it.

        // Once at startAgent() and every 12h after (re-armed by
        // scheduleUpdateCheck() itself) — frequent enough that a new
        // release reaches an idle device within a day, infrequent enough
        // to not be worth a user-facing "check now" control.
        private const val UPDATE_CHECK_INTERVAL_MS = 12 * 60 * 60 * 1000L

        fun start(context: Context) {
            val intent = Intent(context, CameraAgentService::class.java)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) context.startForegroundService(intent)
            else context.startService(intent)
        }

        fun stop(context: Context) {
            context.startService(Intent(context, CameraAgentService::class.java).setAction(ACTION_STOP))
        }

        /**
         * Handles the one cold-start case the instance method
         * [startPairing] cannot: the very first pairing ever on a fresh
         * device, requested before this service exists (or is bound) at
         * all — there's no instance to call a method on yet. Persists
         * [pairing] to [Config] *before* starting the service, since
         * [startAgent] refuses to keep running with zero pairings; stashes
         * [passphrase] in [pendingFirstAttempt] for whichever instance
         * [startAgent] next brings up to actually begin the attempt with.
         */
        fun startFirstPairing(context: Context, pairing: Pairing, passphrase: String) {
            Config.addOrUpdatePairing(context, pairing)
            pendingFirstAttempt = pairing.id to passphrase
            start(context)
        }
    }
}
