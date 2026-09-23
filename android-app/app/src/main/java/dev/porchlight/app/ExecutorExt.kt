package dev.porchlight.app

import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit

/**
 * Submits [task] to this executor, silently dropping it if the executor has
 * already been shut down — safe to call from a foreign thread (WebRTC's
 * native signaling thread, a camera capturer callback, OkHttp's connection
 * thread, a relay's own event-delivery thread) that has no way to know
 * whether [CameraAgentService] has torn this executor down in the meantime.
 * An unguarded `executor.execute { }` from such a thread can throw an
 * uncaught `RejectedExecutionException` racing `restartAgent()`'s
 * `callExecutor.shutdown()`, crashing (restarting — see
 * `PorchlightApplication`) the app. Every foreign-thread or
 * long-delay-scheduled entry point into [NostrSignalingClient]/
 * [WebRtcEngine]/[CameraAgentService]'s shared executor uses this instead of
 * the raw `execute`/`schedule` methods for that reason.
 */
fun ScheduledExecutorService.safeExecute(task: () -> Unit) {
    try {
        execute(task)
    } catch (e: RejectedExecutionException) {
        // Already shut down — whatever this was going to do no longer
        // matters, the owning service is tearing (or has torn) everything
        // down anyway.
    }
}

/** Same reasoning as [safeExecute], for a delayed one-shot task. Returns
 * null (instead of throwing) if the executor was already shut down — the
 * caller treats that exactly like "nothing to cancel later" would anyway. */
fun ScheduledExecutorService.safeSchedule(delay: Long, unit: TimeUnit, task: () -> Unit): java.util.concurrent.ScheduledFuture<*>? =
    try {
        schedule(task, delay, unit)
    } catch (e: RejectedExecutionException) {
        null
    }
