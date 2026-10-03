package dev.porchlight.app

import android.content.Context

/**
 * How long to wait before relaunching after an uncaught exception.
 *
 * A one-off crash restarts a second later, as before. A crash that keeps
 * happening (a native library that won't load, a bad saved state) backs off
 * instead of relaunching every second forever: each further crash within
 * [WINDOW_MS] doubles the wait, up to [MAX_DELAY_MS], and a crash after a
 * quiet [WINDOW_MS] counts as a first one again.
 *
 * Plain Kotlin on purpose, not in the Rust core: a native library that fails
 * to load is the likeliest cause of a startup crash loop, and this has to keep
 * working when it does.
 */
internal object CrashRestartPolicy {
    const val BASE_DELAY_MS = 1_000L
    const val MAX_DELAY_MS = 5 * 60 * 1_000L
    const val WINDOW_MS = 10 * 60 * 1_000L

    private const val PREFS = "crash_restart"
    private const val KEY_TIMES = "times"

    /** Delay before the relaunch for the [crashesInWindow]-th crash in a row (1 = first). */
    fun delayMs(crashesInWindow: Int): Long {
        val doublings = (crashesInWindow - 1).coerceIn(0, 20)
        return minOf(BASE_DELAY_MS shl doublings, MAX_DELAY_MS)
    }

    /** The crash times still inside the window at [now], oldest first. */
    fun withinWindow(times: List<Long>, now: Long): List<Long> = times.filter { now - it in 0..WINDOW_MS }

    /**
     * Records a crash at [now] and returns how long to wait before relaunching.
     * Written with `commit()`, not `apply()`: the process is killed right after.
     */
    fun recordCrashAndDelay(context: Context, now: Long): Long {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val previous = (prefs.getString(KEY_TIMES, "") ?: "").split(',').mapNotNull { it.toLongOrNull() }
        val times = withinWindow(previous, now) + now
        prefs.edit().putString(KEY_TIMES, times.joinToString(",")).commit()
        return delayMs(times.size)
    }
}
