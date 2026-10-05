package dev.porchlight.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioManager
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
 * See `call-core/src/call_arbitration.rs`'s module docs ("Invariants") before
 * changing `hangUp`, `requestCall`,
 * `acceptIncomingCall`, or anything else call-arbitration-related — the
 * single write-up of the invariants that module enforces. This
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
        // Mirrors Pairing.autoAnswer — carried here, not read separately
        // from Config by the UI, so the Settings > Contacts toggle always
        // reflects this running service's own live pairings map instead of
        // whatever the Activity's own Config snapshot last happened to have
        // loaded (which a just-confirmed contact isn't in yet — see
        // setAutoAnswer's own doc).
        val autoAnswer: Boolean = false,
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
    )

    /**
     * The one pairing attempt in progress. It exists only in memory — nothing is saved until both people confirm the
     * match ([confirmPeer]) — and `call-core` owns the protocol state; this is what the screens show. [id] is null
     * until `call-core` has minted it (turning the phrase into the meeting point takes a moment). [candidate] is the
     * peer the SPAKE2 exchange cryptographically confirmed, awaiting the final "Pair with [name]?" tap; there is
     * never more than one — a second distinct sender ends the attempt ([collision]) instead of offering a choice.
     * [timedOut]: the live window elapsed with nobody confirmed. [accepted]: this side's person confirmed and the
     * other person hasn't yet. [notAccepted]: it ended without both confirming (the other cancelled or never did).
     * [completed]: both confirmed and the contact is saved; the screen then goes back to the waiting screen.
     */
    data class PairingAttemptState(
        val id: String? = null,
        val candidate: CandidatePeer? = null,
        val collision: Boolean = false,
        val timedOut: Boolean = false,
        val accepted: Boolean = false,
        val notAccepted: Boolean = false,
        val completed: Boolean = false,
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
        // Only meaningful for NEVER_CONNECTED — see WebRtcEngine.iceDiagnosis.
        val iceDiagnosis: CallCoreBridge.IceDiagnosis? = null,
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
        val pairingAttempt: PairingAttemptState? = null,
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
    @Volatile private var signaling: NostrSignalingClient? = null

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

    // USAGE_NOTIFICATION_RINGTONE alone does NOT route this to the Portal's
    // own built-in speaker — found live on real Portal TV hardware: with
    // only that audio-attribute tag, the ring came out of the connected
    // TV over HDMI, same as the call itself, defeating the entire reason
    // this exists (being audible even when the TV is on a different input
    // — see README's HDMI-CEC note). Nine real-hardware attempts at
    // tagging/routing a MediaPlayer or TextToSpeech output differently
    // all failed identically (2026-10-01, each confirmed via live
    // listening, not just logs): usage alone, usage + setPreferredDevice,
    // USAGE_ALARM, USAGE_ASSISTANCE_ACCESSIBILITY, a granted
    // MANAGE_AUDIO_POLICY permission, a real Notification played by
    // system_server itself, and more. What actually works: the on-device
    // HAL config (/vendor/etc/audio/ripley/audio_policy_configuration.xml)
    // declares the Speaker devicePort at exactly 32000Hz, while HDMI Out
    // is declared at 48000Hz — different capability profiles for the same
    // shared "primary output" mix port. Building a raw AudioTrack that
    // explicitly requests 32000Hz — a rate HDMI's own declared profile
    // doesn't support — makes the platform's routing-candidate logic
    // exclude HDMI, and setPreferredDevice(builtin speaker) actually
    // succeeds instead of being rejected. Confirmed live: the user heard
    // this come out of the Portal itself, not the TV. This is why the
    // ringtone asset itself is baked at 32000Hz (see incoming_call_ring.wav
    // and the scratchpad synthesis script) rather than resampled at
    // runtime — MediaPlayer doesn't expose direct output-sample-rate
    // control the way AudioTrack does, so the file's own native rate is
    // what actually reaches the HAL. This never competes with the call's
    // own audio either way: WebRtcEngine requests no audio focus and,
    // during ringing specifically, is only capturing (recording) locally,
    // not yet playing anything back.
    private var ringtoneTrack: android.media.AudioTrack? = null

    // Puts the incoming-call screen in front of the screensaver and switches
    // the TV input, by pressing Home through the optional accessibility
    // service — see RingScreenGuard.
    private val ringScreenGuard by lazy { RingScreenGuard(this, mainHandler, ::bringToForeground) }

    // True while ringtoneTrack is only the settings screen's volume preview,
    // not a real ring — a real ring arriving mid-preview takes over.
    private var ringtoneIsPreview = false
    private var ringtonePreviewStop: ScheduledFuture<*>? = null

    // Non-null while a ring has changed the system ring volume — what it was
    // before, so restoreRingVolume() puts back exactly that.
    private var ringStreamVolumeBefore: Int? = null

    /**
     * Sets the system ring stream to [level]'s share of its maximum for as
     * long as the ringtone plays, remembering the previous level for
     * [restoreRingVolume]. Setting the real system volume (not just scaling
     * our own AudioTrack) is what lets "High" actually be louder than
     * whatever the Portal happened to be left at — there's no on-screen way
     * to change the ring volume on this hardware. Best effort: Do Not
     * Disturb policy can refuse the change, in which case the ring just plays
     * at whatever the system level already is.
     */
    private fun applyRingVolume(level: Config.RingVolume) {
        val audioManager = getSystemService(AUDIO_SERVICE) as AudioManager
        val max = audioManager.getStreamMaxVolume(AudioManager.STREAM_RING)
        val index = (max * level.fraction + 0.5f).toInt().coerceIn(1, max)
        runCatching {
            if (ringStreamVolumeBefore == null) ringStreamVolumeBefore = audioManager.getStreamVolume(AudioManager.STREAM_RING)
            audioManager.setStreamVolume(AudioManager.STREAM_RING, index, 0)
        }.onFailure { Log.w(TAG, "applyRingVolume: couldn't set the system ring volume", it) }
    }

    private fun restoreRingVolume() {
        val previous = ringStreamVolumeBefore ?: return
        ringStreamVolumeBefore = null
        runCatching { (getSystemService(AUDIO_SERVICE) as AudioManager).setStreamVolume(AudioManager.STREAM_RING, previous, 0) }
    }

    /**
     * Plays the chime briefly at [level] so the settings screen can let
     * someone hear what they just picked. The screen debounces calls, so this
     * only fires once they've stopped cycling. A real incoming call always
     * wins the speaker.
     */
    fun previewRingtone(level: Config.RingVolume) {
        callExecutor?.safeExecute {
            if (_state.value.incomingCall != null) return@safeExecute
            ringtonePreviewStop?.cancel(false)
            stopRingtone()
            if (level == Config.RingVolume.P0) return@safeExecute
            startRingtone(level, preview = true)
            ringtonePreviewStop = callExecutor?.safeSchedule(RINGTONE_PREVIEW_MS, TimeUnit.MILLISECONDS) {
                if (ringtoneIsPreview) stopRingtone()
            }
        }
    }

    private fun startRingtone(level: Config.RingVolume = Config.load(this).ringVolume, preview: Boolean = false) {
        if (ringtoneIsPreview && !preview) {
            ringtonePreviewStop?.cancel(false)
            stopRingtone()
        }
        if (ringtoneTrack != null || level == Config.RingVolume.P0) return
        val sr = 32000
        val pcm = try {
            resources.openRawResource(R.raw.incoming_call_ring).use { input ->
                val all = input.readBytes()
                // Skip the 44-byte canonical WAV header (mono 16-bit PCM,
                // written by the scratchpad synthesis script's plain `wave`
                // module usage — no extra chunks to skip past).
                val mono = ShortArray((all.size - 44) / 2)
                for (i in mono.indices) {
                    val lo = all[44 + i * 2].toInt() and 0xFF
                    val hi = all[44 + i * 2 + 1].toInt()
                    mono[i] = ((hi shl 8) or lo).toShort()
                }
                mono
            }
        } catch (e: Exception) {
            Log.w(TAG, "startRingtone: couldn't read incoming-call ringtone", e)
            return
        }
        val stereo = ShortArray(pcm.size * 2)
        for (i in pcm.indices) {
            stereo[i * 2] = pcm[i]
            stereo[i * 2 + 1] = pcm[i]
        }
        val track = android.media.AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build(),
            )
            .setAudioFormat(
                android.media.AudioFormat.Builder()
                    .setSampleRate(sr)
                    .setEncoding(android.media.AudioFormat.ENCODING_PCM_16BIT)
                    .setChannelMask(android.media.AudioFormat.CHANNEL_OUT_STEREO)
                    .build(),
            )
            // MODE_STATIC needs the buffer sized to the exact data length
            // up front (not AudioTrack.getMinBufferSize()'s small streaming
            // hint) or write() silently only accepts a fraction of it.
            .setBufferSizeInBytes(stereo.size * 2)
            .setTransferMode(android.media.AudioTrack.MODE_STATIC)
            .build()
        val audioManager = getSystemService(AUDIO_SERVICE) as AudioManager
        val speaker = audioManager.getDevices(AudioManager.GET_DEVICES_OUTPUTS)
            .firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
        when {
            speaker == null -> Log.w(TAG, "startRingtone: no TYPE_BUILTIN_SPEAKER output device found; ringtone will follow default routing")
            !track.setPreferredDevice(speaker) -> Log.w(TAG, "startRingtone: setPreferredDevice(builtin speaker) was rejected")
        }
        track.write(stereo, 0, stereo.size)
        track.setLoopPoints(0, pcm.size, -1)
        applyRingVolume(level)
        track.play()
        ringtoneTrack = track
        ringtoneIsPreview = preview
    }

    private fun stopRingtone() {
        ringtoneTrack?.let { it.stop(); it.release() }
        ringtoneTrack = null
        ringtoneIsPreview = false
        restoreRingVolume()
    }

    /** Clears whatever incoming-call-in-progress state exists — safe to call
     * even when there is none. Every path that ends a ringing/counting-down
     * call must go through this so a stale countdown tick can never fire
     * against a call that's already resolved one way or another. */
    private fun clearIncomingCall() {
        incomingCallTickFuture?.cancel(false)
        incomingCallTickFuture = null
        stopRingtone()
        cancelIncomingCallNotification()
        ringScreenGuard.cancel()
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
        when (val result = CallCoreBridge.acceptIncomingCall(System.currentTimeMillis()) ?: return@execute) {
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
     * [Pairing.autoAnswer]'s doc for why this is per-contact, not global.
     * Updates [ContactState.autoAnswer] too, not just [pairings]/[Config] —
     * that's the copy the UI's switch actually reads (see ContactState's own
     * doc), so this takes effect immediately regardless of whether the
     * Activity's own Config snapshot has caught up with this pairing yet. */
    fun setAutoAnswer(pairingId: String, enabled: Boolean) = callExecutor?.execute {
        val updated = pairings[pairingId]?.copy(autoAnswer = enabled) ?: return@execute
        pairings[pairingId] = updated
        Config.addOrUpdatePairing(this, updated)
        updateContact(pairingId) { it.copy(autoAnswer = enabled) }
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
        val result = CallCoreBridge.requestCall(pairingId, ownPubkeyHex, pairing.peerPublicKey, peerOnline, System.currentTimeMillis())
        if (result.callId != null) {
            activePairingId = pairingId
            updateState { it.copy(acceptedIncoming = false) }
        }
        applyCallEffects(result.effects)
    } ?: Unit

    /**
     * A person tapped Confirm on "Pair with [name]?" for the candidate the SPAKE2 exchange already cryptographically
     * confirmed. Pairing needs both people to confirm: `call-core` answers with a waiting screen until the other side
     * has too, then hands back the finished contact ([CallCoreBridge.Effect.PairingComplete]), saved here for the
     * first time.
     */
    fun confirmPeer() = callExecutor?.execute {
        val attempt = _state.value.pairingAttempt ?: return@execute
        val id = attempt.id ?: return@execute
        val candidate = attempt.candidate ?: return@execute
        applyEffects(CallCoreBridge.acceptAttempt(id, candidate.publicKey))
    } ?: Unit

    /**
     * Starts a passphrase pairing attempt for a brand-new contact — the *only* way a pairing is ever created (a
     * stale contact is Delete, then this, with a fresh phrase). [passphrase] and the attempt's fresh keypair are
     * never persisted: `call-core` holds the attempt in memory and hands back the finished contact when a person
     * confirms the match, so an app that dies mid-attempt leaves nothing behind.
     *
     * Turning the phrase into the meeting point (Argon2id) takes a moment, so the attempt shows up in [AgentState]
     * first with no id and gets one when it has started. Everything runs in order on [callExecutor], so a cancel
     * that arrives meanwhile ([discardPairingAttempt]) takes effect right after the start completes. If this service isn't running yet (the first pairing
     * ever on a fresh device), [startFirstPairing] hands the passphrase to the instance that [startAgent] brings up.
     */
    fun startPairing(passphrase: String) {
        val executor = callExecutor
        if (executor != null) {
            executor.execute {
                updateState { it.copy(pairingAttempt = PairingAttemptState()) }
                beginPakeAttempt(passphrase)
            }
        } else {
            pendingFirstAttempt = passphrase
            CameraAgentService.start(this)
        }
    }

    /** Gives up on the attempt in progress (cancel, retry, auto-dismiss). */
    fun discardPairingAttempt() = callExecutor?.execute {
        // A matched attempt tells the other side (a signed cancel), so its person isn't left waiting. A completed one
        // is left to its linger timer, which keeps republishing this side's confirmation for a peer that missed it.
        _state.value.pairingAttempt?.takeUnless { it.completed }?.id?.let { applyEffects(CallCoreBridge.cancelAttempt(it)) }
        updateState { it.copy(pairingAttempt = null) }
    } ?: Unit

    /**
     * Actually starts the SPAKE2 exchange — calls into `CallCoreBridge`, which trims/NFC-normalizes the raw typed
     * passphrase, mints the attempt's id and keypair, derives the rendezvous tag internally (via `pake-bridge`),
     * registers the live attempt in `call-core`'s own registry, and schedules its live-window timeout. Always
     * called on [callExecutor].
     */
    private fun beginPakeAttempt(rawPassphrase: String) {
        val start = CallCoreBridge.startAttempt(Config.load(this).deviceName, rawPassphrase)
        updateState { it.copy(pairingAttempt = PairingAttemptState(id = start.pairingId)) }
        signaling?.kickHeartbeat()
        scheduleAttemptTimer(start.pairingId, start.generation, PAKE_LIVE_WINDOW_MS)
    }

    /**
     * Calls [CallCoreBridge.handleTimeout] after [delayMs]; its generation check makes this a no-op for an attempt
     * that has moved on. safeSchedule, not schedule: this can fire up to a minute or two later, long enough for a
     * restartAgent()/stopAgent() to have shut callExecutor down first (see ExecutorExt.kt's doc).
     */
    private fun scheduleAttemptTimer(pairingId: String, generation: Long, delayMs: Long) {
        callExecutor?.safeSchedule(delayMs, TimeUnit.MILLISECONDS) {
            applyEffects(CallCoreBridge.handleTimeout(pairingId, generation))
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
                is CallCoreBridge.Effect.SendBootstrap ->
                    signaling?.sendPairingBootstrap(effect.ownPrivateKeyHex, effect.rendezvousTag, effect.targetPubkey) {
                        effect.payload.keys().forEach { key -> put(key, effect.payload.get(key)) }
                    }
                CallCoreBridge.Effect.KickHeartbeat -> signaling?.kickHeartbeat()
                is CallCoreBridge.Effect.SetCollision -> updateAttempt(effect.pairingId) { it.copy(candidate = null, collision = true) }
                is CallCoreBridge.Effect.SetTimedOut -> updateAttempt(effect.pairingId) { it.copy(timedOut = true) }
                is CallCoreBridge.Effect.SetConfirmedCandidate ->
                    updateAttempt(effect.pairingId) { it.copy(candidate = CandidatePeer(effect.pubkeyHex, effect.name)) }
                is CallCoreBridge.Effect.SetWaitingForPeer -> {
                    updateAttempt(effect.pairingId) { it.copy(accepted = true) }
                    scheduleAttemptTimer(effect.pairingId, effect.generation, effect.timeoutMs)
                }
                is CallCoreBridge.Effect.SetNotAccepted -> updateAttempt(effect.pairingId) { it.copy(accepted = false, notAccepted = true) }
                is CallCoreBridge.Effect.PairingComplete -> {
                    val pairing = Pairing(effect.pairingId, effect.ownPrivateKeyHex, effect.peerPublicKey, effect.peerName)
                    Config.addOrUpdatePairing(this, pairing)
                    addPairingState(pairing)
                    updateAttempt(effect.pairingId) { it.copy(accepted = false, completed = true) }
                    signaling?.kickHeartbeat()
                    scheduleAttemptTimer(effect.pairingId, effect.generation, effect.lingerMs)
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
     * `call_arbitration` invariant #1: claimed the moment a call *could*
     * happen, not once it's accepted. It's cleared the other way, in
     * [onPeerConnected], not here.
     */
    private fun applyCallEffects(effects: List<CallCoreBridge.CallEffect>) {
        for (effect in effects) {
            when (effect) {
                CallCoreBridge.CallEffect.AcquireMedia -> {
                    engine?.acquireMedia()
                    // A new call is starting (placed or ringing): a leftover
                    // "Call ended"/"Couldn't connect" screen from the previous
                    // one must not stay in front of it. HomeScreen shows a
                    // pending outcome ahead of everything else, so an incoming
                    // call arriving while one was still up (it stays until
                    // dismissed or 20s pass) rang with nothing to see.
                    updateState { it.copy(pendingCallOutcome = null) }
                }
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
                    postIncomingCallNotification(effect.pairingId)
                    ringScreenGuard.ringStarted {
                        cancelIncomingCallNotification()
                        postIncomingCallNotification(effect.pairingId)
                    }
                    startRingtone()
                    updateState { it.copy(incomingCall = IncomingCall(autoAnswer = effect.autoAnswer, secondsRemaining = effect.secondsRemaining)) }
                    if (effect.autoAnswer) scheduleNextCountdownTick(effect.pairingId, effect.callId)
                }
                is CallCoreBridge.CallEffect.SendBye -> signaling?.hangUp(effect.pairingId, effect.callId, effect.mediaFailed)
                CallCoreBridge.CallEffect.ClosePeerConnection -> engine?.closePeer()
                CallCoreBridge.CallEffect.ClearIncomingCallTimer -> clearIncomingCall()
                CallCoreBridge.CallEffect.KickHeartbeat -> signaling?.kickHeartbeat()
                is CallCoreBridge.CallEffect.ShowCallOutcome ->
                    updateState { it.copy(pendingCallOutcome = PendingCallOutcome(effect.pairingId, effect.reason, engine?.iceDiagnosis())) }
            }
        }
    }

    /** For the "Status" screen; null while the agent isn't running. Any thread. */
    fun signalingSnapshot(): NostrSignalingClient.Snapshot? = signaling?.snapshot(System.currentTimeMillis())

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
                    CallCoreBridge.pendingAttemptIds().forEach(CallCoreBridge::cancelAttempt)
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
        if (!config.isValid && pendingFirstAttempt == null) {
            Log.w(TAG, "config invalid; not starting")
            stopSelf()
            return
        }
        startForeground(NOTIF_ID, buildNotification(getString(R.string.notifications_connectingStatus)))
        acquireWakeLock()
        registerScreensaverReceiver()
        ringScreenGuard.register()

        val executor = Executors.newSingleThreadScheduledExecutor()
        callExecutor = executor
        // Before anything connects: the relay list (built in, or the last one fetched) must be in call-core.
        relayListUpdater.init()
        engine = WebRtcEngine(context = this, listener = this, executor = executor).also { it.start() }
        signaling = NostrSignalingClient(context = this, resolver = PairingResolverImpl(), listener = this, executor = executor).also { it.connect() }
        for (pairing in config.pairings) addPairingState(pairing)

        // See startPairing()/pendingFirstAttempt's doc — the very first
        // pairing ever on a fresh device can be requested before this
        // service (and thus callExecutor) exists at all.
        pendingFirstAttempt?.let { passphrase ->
            pendingFirstAttempt = null
            updateState { it.copy(pairingAttempt = PairingAttemptState()) }
            executor.execute { beginPakeAttempt(passphrase) }
        }

        _state.value = _state.value.copy(running = true, statusText = getString(R.string.notifications_connectingStatus))

        scheduleUpdateCheck()
        scheduleRelayListFetch()
    }

    // The relay list: fetched on start, then every RelayListUpdater.FETCH_INTERVAL_MS, when a contact's heartbeat
    // shows a newer one, and when the Status page asks (checkRelayListNow). Applying a new list reconnects.
    private val relayListUpdater = RelayListUpdater(this) { signaling?.refreshRelays() }

    /** The relay list's last check and result, for the Status page. */
    fun relayListStatus(): RelayListUpdater.Status = relayListUpdater.status

    /** The Status page's "Check now". */
    fun checkRelayListNow(onDone: (RelayListUpdater.Result) -> Unit) = relayListUpdater.fetch(force = true, onDone = onDone)

    private fun scheduleRelayListFetch() {
        relayListUpdater.fetch(force = false)
        callExecutor?.safeSchedule(RelayListUpdater.FETCH_INTERVAL_MS, TimeUnit.MILLISECONDS) {
            if (engine != null) scheduleRelayListFetch()
        }
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
            autoAnswer = pairing.autoAnswer,
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
        ringScreenGuard.unregister()
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
        ringScreenGuard.unregister()
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

    private fun updateAttempt(pairingId: String, transform: (PairingAttemptState) -> PairingAttemptState) {
        updateState { state -> state.copy(pairingAttempt = state.pairingAttempt?.takeIf { it.id == pairingId }?.let(transform) ?: state.pairingAttempt) }
    }

    private fun updateContact(pairingId: String, transform: (ContactState) -> ContactState) {
        updateState { state -> state.copy(contacts = state.contacts.map { if (it.id == pairingId) transform(it) else it }) }
    }

    // --- NostrSignalingClient.PairingResolver -------------------------------

    private inner class PairingResolverImpl : NostrSignalingClient.PairingResolver {
        override fun confirmedPeers(): List<NostrSignalingClient.ConfirmedPeer> =
            Config.load(this@CameraAgentService).pairings
                .map { NostrSignalingClient.ConfirmedPeer(it.id, it.ownPrivateKeyHex, it.peerPublicKey, it.lastSignalCreatedAt, it.lastSignalEventId, it.autoAnswer) }

        override fun pendingPairings(): List<NostrSignalingClient.PendingPairing> =
            CallCoreBridge.pendingAttemptIds().mapNotNull { id ->
                val snapshot = CallCoreBridge.pendingSnapshot(id) ?: return@mapNotNull null
                NostrSignalingClient.PendingPairing(
                    pairingId = id,
                    ownPrivateKeyHex = snapshot.ownPrivateKeyHex,
                    rendezvousTag = snapshot.rendezvousTag,
                    bootstrapPayloads = snapshot.payloads,
                    bootstrapTarget = snapshot.candidatePubkey,
                )
            }

        override fun deviceName(): String = Config.load(this@CameraAgentService).deviceName
    }

    // --- NostrSignalingClient.Listener --------------------------------------

    // Required overrides, but nothing in this app currently needs to react
    // to relay-connection transitions specifically (as opposed to a real
    // signal actually being processed, which onRouted below does
    // drive UI from).
    override fun onSignalingConnected() {}

    override fun onSignalingDisconnected() {}

    /**
     * One relay event, already decided by `call-core`'s signal router: the
     * watermark is persisted first, then presence and call effects, then the
     * remaining actions, then any pairing-attempt effects.
     */
    override fun onRouted(result: CallCoreBridge.RouteResult) {
        result.signal?.let { Config.updateLastSignal(this, it.pairingId, it.createdAt, it.eventId) }
        onPresenceUpdate(result.update)
        for (action in result.actions) {
            when (action) {
                is CallCoreBridge.RouteAction.UpdatePeerName -> updatePeerName(action.pairingId, action.name)
                is CallCoreBridge.RouteAction.ApplyRemoteAnswer -> engine?.handleRemoteAnswer(action.sdp)
                is CallCoreBridge.RouteAction.AddRemoteIce -> engine?.addRemoteIce(action.sdpMid, action.sdpMLineIndex, action.candidate)
                // Sent by NostrSignalingClient before this is called.
                is CallCoreBridge.RouteAction.SendHeartbeat -> Unit
            }
        }
        if (result.bootstrapEffects.isNotEmpty()) applyEffects(result.bootstrapEffects)
        if (result.fetchRelayList) relayListUpdater.fetch(force = false)
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
                // Answered inside NostrSignalingClient; filtered out before it gets here.
                is CallCoreBridge.PresenceEffect.ReplyHeartbeat -> Unit
            }
        }
        applyCallEffects(result.callEffects)
    }

    override fun onCallTimeoutCheck(effects: List<CallCoreBridge.CallEffect>) {
        applyCallEffects(effects)
    }

    private fun updatePeerName(pairingId: String, name: String) {
        // The key that matched to get here is still whatever's already
        // pinned for this pairing — only the display label changed.
        val pairing = Config.load(this).pairings.find { it.id == pairingId } ?: return
        if (pairing.peerPublicKey.isBlank() || pairing.peerName == name) return
        Config.updatePairingPeer(this, pairingId, pairing.peerPublicKey, name)
        updateContact(pairingId) { it.copy(name = name) }
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
     * Found live: [bringToForeground]'s plain startActivity() reliably wins
     * against an active ambient dream being dismissed, but loses the race
     * against the system reasserting its own screensaver when waking from a
     * deeper sleep — the incoming call never surfaces. setFullScreenIntent
     * is the OS-designed mechanism for exactly this (the same one phone
     * dialers rely on): it carries priority a plain startActivity() call
     * from a background Service doesn't, specifically to launch over a
     * locked/sleeping screen. Same MainActivity incoming-call screen either
     * way, posted alongside (not instead of) [bringToForeground] since the
     * two are harmless to race — whichever wins, the result is the same
     * Activity single-topped to the front. [clearIncomingCall] cancels this
     * the moment the call stops ringing, same as [stopRingtone].
     */
    private fun postIncomingCallNotification(pairingId: String) {
        val mgr = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (mgr.getNotificationChannel(INCOMING_CALL_CHANNEL_ID) == null) {
            mgr.createNotificationChannel(
                NotificationChannel(INCOMING_CALL_CHANNEL_ID, getString(R.string.notifications_incomingCallChannelName), NotificationManager.IMPORTANCE_HIGH)
                    .apply { description = getString(R.string.notifications_incomingCallChannelDesc) },
            )
        }
        val fullScreenIntent = Intent(this, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pendingIntent = PendingIntent.getActivity(
            this, 0, fullScreenIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val contactName = _state.value.contacts.find { it.id == pairingId }?.name
        val notification = Notification.Builder(this, INCOMING_CALL_CHANNEL_ID)
            .setContentTitle(getString(R.string.notifications_incomingCallTitle))
            .apply { if (contactName != null) setContentText(contactName) }
            .setSmallIcon(android.R.drawable.presence_video_online)
            .setCategory(Notification.CATEGORY_CALL)
            .setFullScreenIntent(pendingIntent, true)
            .setContentIntent(pendingIntent)
            .setAutoCancel(true)
            .build()
        mgr.notify(INCOMING_CALL_NOTIF_ID, notification)
    }

    private fun cancelIncomingCallNotification() {
        (getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager).cancel(INCOMING_CALL_NOTIF_ID)
    }

    /** The person pressed Home while the app was in front: hang up, unless it was this app's own press. */
    fun onUserLeaveHint() {
        if (ringScreenGuard.pressedHomeRecently()) return
        hangUp()
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

    override fun onVideoUnavailable() {
        updateState { it.copy(videoEnabled = false) }
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
        private const val INCOMING_CALL_CHANNEL_ID = "incoming_call"
        private const val INCOMING_CALL_NOTIF_ID = 2

        // How long the settings screen's volume preview plays before stopping
        // itself — about one pass of the chime.
        private const val RINGTONE_PREVIEW_MS = 3_000L
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
        private var pendingFirstAttempt: String? = null

        // "At least 120 seconds" — long enough for two people to type a few
        // words over a call on a TV remote/on-screen keyboard without
        // feeling rushed, short enough not to sit exposed indefinitely. A
        // live pairing attempt that hasn't resolved (crypto-confirmed or
        // collided) by then times out. Read from
        // CallCoreBridge.protocolConstants, not a hand-copied literal.
        private val PAKE_LIVE_WINDOW_MS = CallCoreBridge.protocolConstants.pakeLiveWindowMs

        // AUTO_ANSWER_COUNTDOWN_SECONDS moved into call-core (see
        // call-core's StartRinging effect, which carries
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
        fun startFirstPairing(context: Context, passphrase: String) {
            pendingFirstAttempt = passphrase
            start(context)
        }
    }
}
