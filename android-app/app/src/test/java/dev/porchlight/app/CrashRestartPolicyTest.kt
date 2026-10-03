package dev.porchlight.app

import org.junit.Assert.assertEquals
import org.junit.Test

class CrashRestartPolicyTest {
    @Test
    fun firstCrashRestartsAfterTheBaseDelay() {
        assertEquals(1_000L, CrashRestartPolicy.delayMs(1))
    }

    @Test
    fun repeatedCrashesDoubleTheDelay() {
        assertEquals(listOf(1_000L, 2_000L, 4_000L, 8_000L, 16_000L), (1..5).map { CrashRestartPolicy.delayMs(it) })
    }

    @Test
    fun delayIsCappedAndNeverOverflows() {
        assertEquals(CrashRestartPolicy.MAX_DELAY_MS, CrashRestartPolicy.delayMs(30))
        assertEquals(CrashRestartPolicy.MAX_DELAY_MS, CrashRestartPolicy.delayMs(1_000))
    }

    @Test
    fun nonsenseCountsFallBackToTheBaseDelay() {
        assertEquals(1_000L, CrashRestartPolicy.delayMs(0))
        assertEquals(1_000L, CrashRestartPolicy.delayMs(-5))
    }

    @Test
    fun onlyRecentCrashesCount() {
        val now = 100 * 60_000L
        val times = listOf(now - CrashRestartPolicy.WINDOW_MS - 1, now - CrashRestartPolicy.WINDOW_MS, now - 5_000L)
        assertEquals(listOf(now - CrashRestartPolicy.WINDOW_MS, now - 5_000L), CrashRestartPolicy.withinWindow(times, now))
    }

    @Test
    fun crashTimesFromTheFutureAreDropped() {
        val now = 100 * 60_000L
        assertEquals(emptyList<Long>(), CrashRestartPolicy.withinWindow(listOf(now + 1), now))
    }
}
