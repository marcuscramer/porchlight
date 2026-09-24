package dev.porchlight.app

import android.content.Context
import android.util.Log
import org.webrtc.AudioSource
import org.webrtc.AudioTrack
import org.webrtc.Camera2Enumerator
import org.webrtc.CameraVideoCapturer
import org.webrtc.DefaultVideoDecoderFactory
import org.webrtc.DefaultVideoEncoderFactory
import org.webrtc.EglBase
import org.webrtc.IceCandidate
import org.webrtc.MediaConstraints
import org.webrtc.MediaStreamTrack
import org.webrtc.PeerConnection
import org.webrtc.PeerConnectionFactory
import org.webrtc.SdpObserver
import org.webrtc.SessionDescription
import org.webrtc.SurfaceTextureHelper
import org.webrtc.VideoSink
import org.webrtc.VideoSource
import org.webrtc.VideoTrack

/**
 * Owns the camera + microphone capture and the (at most one) PeerConnection
 * to the other device. Symmetric: either side can initiate — which one
 * calls [createOffer] vs. answers via [handleRemoteOffer] is decided
 * entirely by `call-core`'s own pubkey tie-break (`call_arbitration`'s own
 * doc); this class just reacts to whichever `CallEffect` arrives. Two-way
 * from the start — we send our camera + mic AND render whatever video the
 * remote peer sends back.
 *
 * Audio playback is automatic (libwebrtc routes it to the device speaker);
 * video is not — the caller must attach a VideoSink via [attachRemoteView]
 * for anything to actually show up on screen.
 */
class WebRtcEngine(
    private val context: Context,
    private val listener: Listener,
    /**
     * The same single-threaded executor [NostrSignalingClient] uses,
     * shared and lifecycle-owned by CameraAgentService. libwebrtc's own
     * `PeerConnection.Observer`/`SdpObserver` callbacks fire on its own
     * native signaling thread — not a thread this app controls — so every
     * one of them is marshaled onto this executor before touching [pc] or
     * calling back into [listener]. That's what makes a plain
     * (non-volatile, non-atomic) [pc] safe: one thread owns every write
     * and every read.
     */
    private val executor: java.util.concurrent.ScheduledExecutorService,
    private val captureW: Int = 1280,
    private val captureH: Int = 720,
    private val captureFps: Int = 30,
) {
    interface Listener {
        // pairingId/callId are passed explicitly — captured by this class
        // at newPeerConnection() time (see pcPairingId/pcCallId's own doc).
        // Reading a value captured once when negotiation started, instead
        // of a live mutable field an async callback might race, is a real
        // correctness property.
        fun onLocalOffer(pairingId: String, callId: String, sdp: String)
        fun onLocalAnswer(pairingId: String, callId: String, sdp: String)
        fun onLocalIce(pairingId: String, callId: String, candidate: IceCandidate)
        /** [pairingId]/[callId] are `null` only if a peer connection
         * somehow closed before ever being associated with one (shouldn't
         * happen in practice, but this class fails safe rather than lying
         * about it). [callId] is needed (added alongside the ShowCallOutcome
         * work) so [CallCoreBridge.markConnected] can be called with the
         * call this transition is actually about, not just which pairing. */
        fun onPeerConnected(pairingId: String?, callId: String?, connected: Boolean)
        fun onCapturingChanged(capturing: Boolean)
    }

    data class IceServer(val urls: String, val username: String? = null, val credential: String? = null)

    val eglBase: EglBase = EglBase.create()

    private lateinit var factory: PeerConnectionFactory
    private var videoCapturer: CameraVideoCapturer? = null
    private var surfaceHelper: SurfaceTextureHelper? = null
    private var videoSource: VideoSource? = null
    private var audioSource: AudioSource? = null
    private var localVideoTrack: VideoTrack? = null
    private var localAudioTrack: AudioTrack? = null

    // Not @Volatile: every write and every read is marshaled onto
    // [executor] (see the class doc), so single-writer confinement is what
    // makes this safe.
    private var pc: PeerConnection? = null
    /** Which pairing/call the current [pc] belongs to — set once in
     * [newPeerConnection], read by every native-callback closure for that
     * `pc`'s whole lifetime, cleared in [cleanupPeerConnection]. */
    private var pcPairingId: String? = null
    private var pcCallId: String? = null

    private var iceServers: List<PeerConnection.IceServer> = emptyList()

    private var localPreviewSink: VideoSink? = null
    private var remoteViewSink: VideoSink? = null
    private var remoteVideoTrack: VideoTrack? = null

    private var started = false
    private var capturing = false

    /** Build the factory and immediately open the camera + mic. */
    fun start(servers: List<IceServer> = defaultIceServers()) {
        if (started) return
        started = true
        iceServers = servers.map { s ->
            PeerConnection.IceServer.builder(s.urls)
                .setUsername(s.username ?: "")
                .setPassword(s.credential ?: "")
                .createIceServer()
        }

        // PeerConnectionFactory.initialize(...) already happened once in
        // PorchlightApplication.onCreate() — see its doc for why that's
        // where this now lives instead of here.
        val encoder = DefaultVideoEncoderFactory(eglBase.eglBaseContext, true, true)
        val decoder = DefaultVideoDecoderFactory(eglBase.eglBaseContext)
        factory = PeerConnectionFactory.builder()
            .setVideoEncoderFactory(encoder)
            .setVideoDecoderFactory(decoder)
            .createPeerConnectionFactory()

        // Camera/mic are NOT acquired here — only once a call actually starts
        // (see newPeerConnection()) and released the moment it ends (see
        // cleanupPeerConnection()). A device sitting registered and reachable
        // shouldn't mean its camera is live 24/7.
    }

    /**
     * Open the camera + mic and build the local tracks. Idempotent.
     *
     * Called from two places now: [newPeerConnection] once negotiation
     * actually starts, and directly from `CameraAgentService.requestCall`
     * the moment a human taps "Call" — even before the peer is known to be
     * reachable, since the calling screen shows a self-view immediately
     * ("connecting might take some time," not "nothing happens until they
     * answer"). Whichever happens first wins; the second call is a no-op.
     */
    @Synchronized
    fun acquireMedia() {
        if (capturing) return
        val enumerator = Camera2Enumerator(context)
        val names = enumerator.deviceNames
        // Portal has a single (front-facing) camera; prefer it, else take whatever exists.
        val deviceName = names.firstOrNull { enumerator.isFrontFacing(it) }
            ?: names.firstOrNull()
            ?: run { Log.e(TAG, "no camera found"); return }

        val capturer = enumerator.createCapturer(deviceName, cameraEventsHandler())
        videoCapturer = capturer
        surfaceHelper = SurfaceTextureHelper.create("CaptureThread", eglBase.eglBaseContext)
        videoSource = factory.createVideoSource(false)
        capturer.initialize(surfaceHelper, context, videoSource!!.capturerObserver)
        capturer.startCapture(captureW, captureH, captureFps)

        localVideoTrack = factory.createVideoTrack(VIDEO_ID, videoSource).apply {
            setEnabled(true)
            localPreviewSink?.let { addSink(it) }
        }

        audioSource = factory.createAudioSource(MediaConstraints())
        localAudioTrack = factory.createAudioTrack(AUDIO_ID, audioSource).apply { setEnabled(true) }

        capturing = true
        Log.i(TAG, "media acquired (camera on)")
        listener.onCapturingChanged(true)
    }

    /** Real mute, not just hiding the self-view preview (PreviewCorner.
     * INVISIBLE is that, a separate concern) — disabling a track makes it
     * emit silence/black frames on its own (WebRTC's own defined behavior),
     * so this needs no renegotiation and mutes for the peer too. A no-op
     * before acquireMedia() has run; the next acquireMedia() always starts
     * a fresh track enabled. */
    fun setAudioEnabled(enabled: Boolean) { localAudioTrack?.setEnabled(enabled) }
    fun setVideoEnabled(enabled: Boolean) { localVideoTrack?.setEnabled(enabled) }

    /**
     * A camera can be forcibly killed mid-call by something outside our
     * control (a device-admin camera-disable policy, observed alongside
     * Portal's own presence-detection screensaver taking over). Without
     * this handler, WebRTC's own PeerConnectionState never reflects that —
     * ICE/DTLS stays CONNECTED since only the camera died, not the
     * transport — so nothing else in this class would ever notice. This
     * treats a camera failure as a real disconnect, the same as the peer
     * leaving, so it self-corrects instead of needing someone to notice a
     * black screen and act.
     */
    private fun cameraEventsHandler() = object : CameraVideoCapturer.CameraEventsHandler {
        override fun onCameraError(errorDescription: String) = onMediaFailure("Camera error: $errorDescription")
        override fun onCameraDisconnected() = onMediaFailure("Camera disconnected")
        override fun onCameraFreezed(errorDescription: String) = onMediaFailure("Camera frozen: $errorDescription")
        override fun onCameraOpening(cameraName: String) {}
        override fun onFirstFrameAvailable() {}
        override fun onCameraClosed() {}
    }

    private fun onMediaFailure(message: String) {
        // Logged for developer diagnostics only — no user-facing display;
        // what matters is that a broken camera correctly ends an active
        // call below.
        Log.e(TAG, message)
        // Deferred onto the shared executor: this callback can fire from
        // the capturer's own internal thread, and cleanupPeerConnection
        // ultimately calls back into releaseMedia()/stopCapture() — better
        // not to risk reentering the capturer's own callback machinery
        // synchronously. safeExecute, not execute: this foreign thread has
        // no way to know whether CameraAgentService has already shut this
        // executor down (see ExecutorExt.kt's doc).
        executor.safeExecute {
            // pc == null means media was only ever acquired for a
            // self-view preview during incoming-call ringing (see
            // CameraAgentService.onOffer) — no actual call has been
            // negotiated yet. Tearing down the entire incoming call over a
            // glitched preview would be wrong; just release the broken
            // capture and leave the ring (and the human's chance to still
            // Accept or Decline) alone. Only an *actual* in-progress call
            // (pc != null) still ends outright — decided by call-core's own
            // `should_end_call_on_media_failure` rather than an inline
            // guard here, so web automatically gets the same guarantee once
            // it grows an equivalent trigger.
            if (CallCoreBridge.shouldEndCallOnMediaFailure(pc != null)) closePeer() else releaseMedia()
        }
    }

    @Synchronized
    private fun releaseMedia() {
        if (!capturing) return
        localPreviewSink?.let { localVideoTrack?.removeSink(it) }
        try { videoCapturer?.stopCapture() } catch (_: Exception) {}
        videoCapturer?.dispose(); videoCapturer = null
        surfaceHelper?.dispose(); surfaceHelper = null
        localVideoTrack?.dispose(); localVideoTrack = null
        localAudioTrack?.dispose(); localAudioTrack = null
        videoSource?.dispose(); videoSource = null
        audioSource?.dispose(); audioSource = null
        capturing = false
        Log.i(TAG, "media released (camera off)")
        listener.onCapturingChanged(false)
    }

    /** Attach a local self-view renderer (already init()'d with eglBase context). */
    fun attachLocalPreview(sink: VideoSink) {
        // Remove whatever was attached before adding the new one — found
        // live on real Portal hardware: a second attach without this left
        // the previous (by then invalid/released) renderer's sink
        // permanently registered on the track, since overwriting
        // localPreviewSink loses the only reference anything could have
        // called removeSink with.
        localPreviewSink?.let { localVideoTrack?.removeSink(it) }
        localPreviewSink = sink
        localVideoTrack?.addSink(sink)
    }

    fun detachLocalPreview() {
        localPreviewSink?.let { localVideoTrack?.removeSink(it) }
        localPreviewSink = null
    }

    /** Attach the renderer that shows the remote peer's video (already init()'d). */
    fun attachRemoteView(sink: VideoSink) {
        // Same reasoning as attachLocalPreview above.
        remoteViewSink?.let { remoteVideoTrack?.removeSink(it) }
        remoteViewSink = sink
        remoteVideoTrack?.addSink(sink)
    }

    fun detachRemoteView() {
        remoteViewSink?.let { remoteVideoTrack?.removeSink(it) }
        remoteViewSink = null
    }

    private fun newPeerConnection(pairingId: String, callId: String): PeerConnection? {
        pc?.close()
        acquireMedia()
        val rtcConfig = PeerConnection.RTCConfiguration(iceServers).apply {
            sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
            continualGatheringPolicy = PeerConnection.ContinualGatheringPolicy.GATHER_CONTINUALLY
        }
        val connection = factory.createPeerConnection(rtcConfig, peerObserver()) ?: return null
        pc = connection
        pcPairingId = pairingId
        pcCallId = callId
        val streamIds = listOf(STREAM_ID)
        localVideoTrack?.let { connection.addTrack(it, streamIds) }
        localAudioTrack?.let { connection.addTrack(it, streamIds) }
        return connection
    }

    /**
     * We're the initiator — `call-core` decided so (this side won the
     * pubkey tie-break) — create and send an offer. `createOffer`'s
     * SdpObserver callback fires on libwebrtc's own native thread, not
     * [executor] — marshaled here before touching `connection` again or
     * calling back into [listener]. [pairingId]/[callId] come from the
     * `CallEffect.CreateOffer` that triggered this — captured here for this
     * `pc`'s whole lifetime.
     */
    fun createOffer(pairingId: String, callId: String) {
        val connection = newPeerConnection(pairingId, callId) ?: return
        connection.createOffer(object : SimpleSdpObserver() {
            // safeExecute, not execute: libwebrtc's native thread has no
            // way to know whether CameraAgentService has already shut this
            // executor down (see ExecutorExt.kt's doc).
            override fun onCreateSuccess(desc: SessionDescription) = executor.safeExecute {
                connection.setLocalDescription(SimpleSdpObserver(), desc)
                listener.onLocalOffer(pairingId, callId, desc.description)
            }
        }, MediaConstraints())
    }

    /** The other side sent an offer (we were already waiting): answer it.
     * Same native-thread marshaling as [createOffer]. [pairingId]/[callId]
     * come from whichever `CallEffect` triggered this (`ApplyRemoteOffer`
     * for a fresh ring's accept, or the tie-break fast-path — see
     * `CallCoreBridge`'s doc either way). */
    fun handleRemoteOffer(pairingId: String, callId: String, sdp: String) {
        val connection = newPeerConnection(pairingId, callId) ?: return
        connection.setRemoteDescription(object : SimpleSdpObserver() {
            override fun onSetSuccess() = executor.safeExecute {
                connection.createAnswer(object : SimpleSdpObserver() {
                    override fun onCreateSuccess(desc: SessionDescription) = executor.safeExecute {
                        connection.setLocalDescription(SimpleSdpObserver(), desc)
                        listener.onLocalAnswer(pairingId, callId, desc.description)
                    }
                }, MediaConstraints())
            }
        }, SessionDescription(SessionDescription.Type.OFFER, sdp))
    }

    /**
     * We initiated; the other side answered our offer. The caller
     * (CameraAgentService) is expected to have already checked
     * `CallCoreBridge.shouldApplyAnswer(pairingId, callId)` — the *which
     * call* guard, at the pairing/call-id level. This function's own
     * `answerApplied` guard below is a *second*, independent check at the
     * real WebRTC `signalingState` level, which only updates once
     * `setRemoteDescription`'s async work actually lands — without it, a
     * near-simultaneous duplicate answer could still observe
     * `HAVE_LOCAL_OFFER` and apply itself too, past `call-core`'s own guard
     * (a narrow timing window `call-core` has no visibility into, since it
     * never sees `pc`). A plain boolean, not `AtomicBoolean`: every call
     * into this function and every native callback that could reach `pc`
     * lands on the same one thread ([executor]).
     */
    private var answerApplied = false

    fun handleRemoteAnswer(sdp: String) {
        val connection = pc ?: return
        if (connection.signalingState() != PeerConnection.SignalingState.HAVE_LOCAL_OFFER || answerApplied) return
        answerApplied = true
        connection.setRemoteDescription(SimpleSdpObserver(), SessionDescription(SessionDescription.Type.ANSWER, sdp))
    }

    fun addRemoteIce(sdpMid: String?, sdpMLineIndex: Int, candidate: String) {
        pc?.addIceCandidate(IceCandidate(sdpMid, sdpMLineIndex, candidate))
    }

    /**
     * Drop our references and notify the UI. Never calls
     * PeerConnection.close() synchronously — safe to call from anywhere,
     * including from the connection's own observer callback
     * (onConnectionChange for FAILED can fire spontaneously). The actual
     * native close() is deferred onto [executor] so it never runs
     * re-entrantly on WebRTC's signaling thread — libwebrtc aborts if you
     * close() a PeerConnection from within a callback running on that same
     * thread.
     */
    private fun cleanupPeerConnection() {
        val closing = pc
        val closingPairingId = pcPairingId
        val closingCallId = pcCallId
        remoteViewSink?.let { remoteVideoTrack?.removeSink(it) }
        remoteVideoTrack = null
        pc = null
        pcPairingId = null
        pcCallId = null
        answerApplied = false
        // Telling call-core's own bookkeeping this real PeerConnection is
        // gone (CallCoreBridge.peerConnectionClosed()) lives in
        // CameraAgentService.onPeerConnected instead of here — that call
        // can return a CallEffect.ShowCallOutcome, which only the shell's
        // own effect-applying layer knows how to act on.
        listener.onPeerConnected(closingPairingId, closingCallId, false)
        releaseMedia()
        if (closing != null) executor.safeExecute { closing.close() }
    }

    /** The other device disconnected, or we're ending the call ourselves. */
    fun closePeer() = cleanupPeerConnection()

    fun stop() {
        cleanupPeerConnection() // also releases media, if a call was active
        detachLocalPreview()
        detachRemoteView()
        if (started) factory.dispose()
        eglBase.release()
        started = false
    }

    // Every callback here fires on libwebrtc's own native signaling thread,
    // not [executor] — each one is marshaled before touching `pc`/
    // `remoteVideoTrack` or calling back into [listener]. safeExecute, not
    // execute: that native thread has no way to know whether
    // CameraAgentService has already shut this executor down (see
    // ExecutorExt.kt's doc).
    private fun peerObserver() = object : PeerConnection.Observer {
        override fun onIceCandidate(candidate: IceCandidate) = executor.safeExecute {
            val id = pcPairingId
            val callId = pcCallId
            if (id != null && callId != null) listener.onLocalIce(id, callId, candidate)
        }
        override fun onConnectionChange(newState: PeerConnection.PeerConnectionState) = executor.safeExecute {
            Log.d(TAG, "peer connection: $newState")
            when (newState) {
                PeerConnection.PeerConnectionState.CONNECTED -> listener.onPeerConnected(pcPairingId, pcCallId, true)
                PeerConnection.PeerConnectionState.FAILED,
                PeerConnection.PeerConnectionState.CLOSED -> closePeer()
                else -> {}
            }
        }
        // Audio plays automatically once enabled. Video needs an explicit sink.
        override fun onAddTrack(receiver: org.webrtc.RtpReceiver?, streams: Array<out org.webrtc.MediaStream>?) = executor.safeExecute {
            val track = receiver?.track() as? MediaStreamTrack ?: return@safeExecute
            track.setEnabled(true)
            if (track is VideoTrack) {
                remoteVideoTrack = track
                remoteViewSink?.let { track.addSink(it) }
            }
        }
        override fun onSignalingChange(state: PeerConnection.SignalingState?) {}
        override fun onIceConnectionChange(state: PeerConnection.IceConnectionState?) {}
        override fun onIceConnectionReceivingChange(receiving: Boolean) {}
        override fun onIceGatheringChange(state: PeerConnection.IceGatheringState?) {}
        override fun onIceCandidatesRemoved(candidates: Array<out IceCandidate>?) {}
        override fun onAddStream(stream: org.webrtc.MediaStream?) {}
        override fun onRemoveStream(stream: org.webrtc.MediaStream?) {}
        override fun onDataChannel(channel: org.webrtc.DataChannel?) {}
        override fun onRenegotiationNeeded() {}
    }

    /** SdpObserver with all methods defaulted; override what you need. */
    open class SimpleSdpObserver : SdpObserver {
        override fun onCreateSuccess(desc: SessionDescription) {}
        override fun onSetSuccess() {}
        override fun onCreateFailure(error: String?) { Log.w(TAG, "createSDP failed: $error") }
        override fun onSetFailure(error: String?) { Log.w(TAG, "setSDP failed: $error") }
    }

    companion object {
        private const val TAG = "WebRtcEngine"
        private const val STREAM_ID = "porchlight"
        private const val VIDEO_ID = "video0"
        private const val AUDIO_ID = "audio0"

        /**
         * STUN only, no TURN — fine as long as both devices' NATs allow a
         * direct path (confirmed for this deployment via a real cross-network
         * test client call). If a future network change makes that stop
         * working, you'll see it on the waiting screen as "Other device
         * online" staying green while "Call connected" stays red.
         *
         * To add TURN when that happens: get a relay (e.g. Metered's Open
         * Relay, metered.ca — free tier, no self-hosting) and append its
         * servers here, e.g.:
         *   defaultIceServers() + IceServer("turn:host:3478", user, pass)
         * Fetch fresh credentials at startup rather than hardcoding them if
         * the provider issues time-limited ones.
         */
        fun defaultIceServers(): List<IceServer> = listOf(
            IceServer("stun:stun.l.google.com:19302"),
            IceServer("stun:stun1.l.google.com:19302"),
        )
    }
}
