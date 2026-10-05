package dev.porchlight.app

import android.content.Context
import android.util.Log
import java.io.IOException
import okhttp3.Call
import okhttp3.Callback
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response

/**
 * Keeps the relay list current: the list this build ships with is only the starting point. The project serves
 * `relays.json` next to its web page (GitHub Pages); this fetches it on start, every [FETCH_INTERVAL_MS], when a
 * contact's heartbeat shows a newer version, and when the Status page asks. `call-core`'s `relay_list` decides
 * whether the text is acceptable and newer; this only fetches, remembers the last good text across restarts, and
 * tells [onApplied] when the active list changed. The URL is derived from [BuildConfig.UPDATE_REPO], the same
 * repository the app updates itself from; a build without one only ever uses its built-in list.
 *
 * Runs on OkHttp's own threads; call-core's state is behind its own lock, so that is safe.
 */
class RelayListUpdater(private val context: Context, private val onApplied: () -> Unit) {
    sealed interface Result {
        data object UpToDate : Result
        data class Applied(val version: Int) : Result
        data class Failed(val reason: String) : Result
        /** This build has no update repository, so there is nothing to fetch. */
        data object Disabled : Result
    }

    /** What the Status page shows: when the list was last checked and how that went. */
    data class Status(val checkedAtMs: Long?, val result: Result?)

    @Volatile var status: Status = Status(null, null)
        private set

    private val prefs get() = context.getSharedPreferences("relay_list", Context.MODE_PRIVATE)
    private val http = OkHttpClient()
    @Volatile private var lastStartedMs = 0L

    /** Sets the built-in list, then lets the last fetched list (if any, and newer) replace it. Call before relays are used. */
    fun init() {
        CallCoreBridge.relayListInit(GeneratedSharedConfig.RELAYS_VERSION, GeneratedSharedConfig.RELAYS)
        prefs.getString(KEY_JSON, null)?.let { CallCoreBridge.relayListApply(it, System.currentTimeMillis()) }
        val at = prefs.getLong(KEY_CHECKED_AT, 0L)
        if (at > 0) status = Status(at, null)
    }

    private fun url(): String? {
        val repo = BuildConfig.UPDATE_REPO.trim()
        val (owner, name) = repo.split("/").takeIf { it.size == 2 && it.all(String::isNotBlank) } ?: return null
        return "https://$owner.github.io/$name/relays.json"
    }

    /**
     * Fetches the list now. [force] is the Status page's button: it asks the CDN for a fresh copy (a throwaway query
     * parameter and no-cache) and ignores clicks less than [MIN_FORCED_GAP_MS] apart. Otherwise it is an ordinary
     * conditional-friendly request. [onDone] gets the result on a background thread.
     */
    fun fetch(force: Boolean, onDone: ((Result) -> Unit)? = null) {
        val base = url()
        if (base == null) {
            record(Result.Disabled)
            onDone?.invoke(Result.Disabled)
            return
        }
        val now = System.currentTimeMillis()
        if (force && now - lastStartedMs < MIN_FORCED_GAP_MS) return
        lastStartedMs = now
        val request = Request.Builder()
            .url(if (force) "$base?t=$now" else base)
            .apply { if (force) header("Cache-Control", "no-cache") }
            .build()
        http.newCall(request).enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) = finish(Result.Failed(e.message ?: e.javaClass.simpleName), onDone)

            override fun onResponse(call: Call, response: Response) {
                response.use {
                    if (!it.isSuccessful) return finish(Result.Failed("HTTP ${it.code}"), onDone)
                    val body = it.body?.string()?.take(MAX_BODY_CHARS)
                    if (body.isNullOrBlank()) return finish(Result.Failed("empty reply"), onDone)
                    when (val outcome = CallCoreBridge.relayListApply(body, System.currentTimeMillis())) {
                        is CallCoreBridge.RelayListOutcome.Applied -> {
                            prefs.edit().putString(KEY_JSON, body).apply()
                            finish(Result.Applied(outcome.version), onDone)
                            Log.i(TAG, "relay list v${outcome.version} applied (+${outcome.added.size} -${outcome.removed.size})")
                            onApplied()
                        }
                        is CallCoreBridge.RelayListOutcome.Unchanged -> finish(Result.UpToDate, onDone)
                        is CallCoreBridge.RelayListOutcome.Rejected -> finish(Result.Failed(outcome.reason), onDone)
                    }
                }
            }
        })
    }

    private fun finish(result: Result, onDone: ((Result) -> Unit)?) {
        record(result)
        onDone?.invoke(result)
    }

    private fun record(result: Result) {
        // "Checked" means a check that got an answer; a failed or impossible one leaves the last good time alone.
        if (result is Result.Failed || result is Result.Disabled) {
            status = Status(status.checkedAtMs, result)
            return
        }
        val now = System.currentTimeMillis()
        status = Status(now, result)
        prefs.edit().putLong(KEY_CHECKED_AT, now).apply()
    }

    companion object {
        private const val TAG = "RelayListUpdater"
        private const val KEY_JSON = "json"
        private const val KEY_CHECKED_AT = "checked_at"
        const val FETCH_INTERVAL_MS = 6L * 60 * 60 * 1000
        private const val MIN_FORCED_GAP_MS = 10_000L
        private const val MAX_BODY_CHARS = 16_384
    }
}
