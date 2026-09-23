package dev.porchlight.app

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

/** Where the local self-view preview sits during a call — cycled by the
 * "Position self-view" call-control button (HomeScreens.kt's CallScreen).
 * [INVISIBLE] hides the preview widget only — it has no effect on the
 * outgoing video track itself (see AgentState.videoEnabled for the real
 * mute). Persisted so it sticks across restarts. Declaration order is
 * cycle order (see next()) — mirrors web's identical PREVIEW_POSITIONS
 * (app.js) control-for-control. */
enum class PreviewCorner {
    BOTTOM_START, TOP_START, TOP_END, BOTTOM_END, INVISIBLE;

    fun next(): PreviewCorner = entries[(ordinal + 1) % entries.size]
}

/**
 * One contact. [id] is a stable local identifier independent of
 * [ownPrivateKeyHex] — post-compromise recovery is Delete + Add contact (a
 * brand-new [id] each time; see CameraAgentService.startPairing), not an
 * in-place update of this same record.
 *
 * [ownPrivateKeyHex] is *this device's own* permanent Nostr identity for
 * *this one relationship* — generated fresh the moment a pairing attempt
 * starts and never reused across pairings or shared with the peer: every
 * `Pairing` is a fully independent cryptographic relationship, so
 * compromising one never exposes any other. [peerPublicKey] is the *peer's*
 * Nostr pubkey (hex) — populated once the SPAKE2 exchange succeeds and a
 * human confirms the peer's self-reported name (see
 * CameraAgentService.confirmPeer).
 */
data class Pairing(
    val id: String,
    val ownPrivateKeyHex: String,
    // "" (not null) for both — simpler JSON round-tripping than a nullable field.
    val peerPublicKey: String = "",
    val peerName: String = "",
    // Per-contact, off by default (see CameraAgentService.onOffer): when
    // true, an incoming call from this contact connects automatically after
    // a countdown instead of waiting for a manual Accept tap — a deliberate
    // per-contact choice, not a device-wide one.
    val autoAnswer: Boolean = false,
    // The newest gift-wrapped signal (by created_at, then by id — see
    // WrapEventCandidate.lastSignalCreatedAt's own doc, CallCoreBridge.kt,
    // for why both are needed) this pairing has actually processed — 0L/""
    // for one that's never received any yet. Persisted here so
    // CallCoreBridge.unwrapWrappedEventForAny can recognize a relay
    // redelivery even across a reload/app restart.
    val lastSignalCreatedAt: Long = 0L,
    val lastSignalEventId: String = "",
) {
    val isConfirmed: Boolean get() = peerPublicKey.isNotBlank()
}

/**
 * Persisted configuration for the call app: a name for this device, and the
 * list of contacts it currently has. A device can have several simultaneous
 * confirmed pairings, each tracked independently.
 */
data class Config(
    val deviceName: String = "",
    val pairings: List<Pairing> = emptyList(),
    val previewCorner: PreviewCorner = PreviewCorner.BOTTOM_START,
) {
    /** True once a name has been entered — gates the first-launch name screen. */
    val hasName: Boolean get() = deviceName.isNotBlank()

    /** True once this device has at least one pairing — the point where
     * onboarding is actually done and HomeScreen has something to show. */
    val isValid: Boolean get() = pairings.isNotEmpty()

    companion object {
        private const val PREFS = "portal_call"

        // Every mutator below does load-modify-save as separate steps, not
        // one atomic op — CameraAgentService's signaling runs on its own
        // background thread(s), so two pairings can genuinely call a
        // mutator at the same instant. Without this lock, two interleaved
        // load-modify-saves silently lose whichever one wrote second's
        // *base* snapshot — e.g. pairing A's confirmPeer pin vanishing
        // because pairing B's own mutator loaded Config before A's save
        // landed, then saved its own change back over it. Confirmed
        // on-device with two simultaneous pairing confirmations.
        private val lock = Any()

        fun load(context: Context): Config = synchronized(lock) {
            val p = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val deviceName = (p.getString("deviceName", "") ?: "").ifBlank { BuildConfig.DEFAULT_DEVICE_NAME }
            val pairings = parsePairings(p.getString("pairings", null))
            val previewCorner = runCatching {
                PreviewCorner.valueOf(p.getString("previewCorner", null) ?: "")
            }.getOrDefault(PreviewCorner.BOTTOM_START)

            Config(deviceName, pairings, previewCorner)
        }

        fun save(context: Context, config: Config): Unit = synchronized(lock) {
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
                .putString("deviceName", config.deviceName)
                .putString("pairings", serializePairings(config.pairings))
                .putString("previewCorner", config.previewCorner.name)
                .apply()
        }

        /** Always re-loads fresh and writes back under [lock], so no mutator
         * below can clobber a concurrent write elsewhere by merging onto a
         * stale snapshot — see [lock]'s own doc for the confirmed on-device
         * failure this prevents. The one place all three mutators' shared
         * load-transform-save shape lives now, instead of each hand-copying
         * it. */
        private fun mutate(context: Context, transform: (Config) -> Config): Unit = synchronized(lock) {
            save(context, transform(load(context)))
        }

        /** Adds a brand-new pairing, or replaces an existing one with the
         * same id. */
        fun addOrUpdatePairing(context: Context, pairing: Pairing) = mutate(context) { config ->
            config.copy(pairings = config.pairings.filterNot { it.id == pairing.id } + pairing)
        }

        /** A human confirmed this pairing's candidate — pin it. */
        fun updatePairingPeer(context: Context, pairingId: String, publicKeyHex: String, name: String) = mutate(context) { config ->
            config.copy(pairings = config.pairings.map {
                if (it.id == pairingId) it.copy(peerPublicKey = publicKeyHex, peerName = name) else it
            })
        }

        /** Called from CameraAgentService right after a wrap event is
         * actually accepted (see Pairing.lastSignalCreatedAt's own doc) —
         * persists the new high-water mark immediately, not batched, so it
         * survives a reload/restart even if the very next thing that
         * happens is a crash. */
        fun updateLastSignal(context: Context, pairingId: String, createdAt: Long, eventId: String) = mutate(context) { config ->
            config.copy(pairings = config.pairings.map {
                if (it.id == pairingId) it.copy(lastSignalCreatedAt = createdAt, lastSignalEventId = eventId) else it
            })
        }

        /** "Forget this contact" — deletes the pairing entirely. Not a
         * security remedy: leaves this device's own identity and every
         * *other* pairing untouched. */
        fun removePairing(context: Context, pairingId: String) = mutate(context) { config ->
            config.copy(pairings = config.pairings.filterNot { it.id == pairingId })
        }

        private fun parsePairings(json: String?): List<Pairing> {
            if (json.isNullOrBlank()) return emptyList()
            return runCatching {
                val array = JSONArray(json)
                (0 until array.length()).map { i ->
                    val o = array.getJSONObject(i)
                    Pairing(
                        id = o.getString("id"),
                        ownPrivateKeyHex = o.getString("ownPrivateKeyHex"),
                        peerPublicKey = o.optString("peerPublicKey"),
                        peerName = o.optString("peerName"),
                        autoAnswer = o.optBoolean("autoAnswer", false),
                        lastSignalCreatedAt = o.optLong("lastSignalCreatedAt", 0L),
                        lastSignalEventId = o.optString("lastSignalEventId"),
                    )
                }
            }.getOrDefault(emptyList())
        }

        private fun serializePairings(pairings: List<Pairing>): String {
            val array = JSONArray()
            for (pairing in pairings) {
                array.put(
                    JSONObject()
                        .put("id", pairing.id)
                        .put("ownPrivateKeyHex", pairing.ownPrivateKeyHex)
                        .put("peerPublicKey", pairing.peerPublicKey)
                        .put("peerName", pairing.peerName)
                        .put("autoAnswer", pairing.autoAnswer)
                        .put("lastSignalCreatedAt", pairing.lastSignalCreatedAt)
                        .put("lastSignalEventId", pairing.lastSignalEventId),
                )
            }
            return array.toString()
        }
    }
}
