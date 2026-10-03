package dev.porchlight.app

import android.app.Activity
import android.app.Application
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import java.util.concurrent.atomic.AtomicInteger

/**
 * Gets the incoming-call screen in front of the Portal's screensaver, and
 * switches the TV to the Portal's input, by pressing the Home key through
 * [CallWakeUpAccessibilityService].
 *
 * Why Home: after a wake from deep sleep the screensaver comes up on its own
 * and wins against any activity the app starts, for the whole ring. A Home
 * press ends it and keeps it from coming back, and the system's own handling
 * of a short Home press also makes the Portal send the HDMI-CEC messages that
 * switch the TV to its input. A Home press does nothing while the display is
 * still off, so this waits until the device is interactive.
 *
 * After the press the app brings itself back to the front (Home takes it
 * off the front when it was already there), then checks that its screen is
 * resumed and the screensaver is not running. It presses Home again only if
 * the screensaver is back (a second press on the system's home screen starts
 * the screensaver), and otherwise just asks to come to the front again, for
 * up to [GIVE_UP_MS]. Without the accessibility service enabled it does a
 * plain bring-to-front.
 */
internal class RingScreenGuard(
    private val context: Context,
    private val handler: Handler,
    private val bringToFront: () -> Unit,
) {
    private val power = context.getSystemService(Context.POWER_SERVICE) as PowerManager

    // Bumped on every start and cancel; a step scheduled for an older ring
    // sees a different value and does nothing.
    private val ringId = AtomicInteger(0)

    @Volatile private var dreaming = false
    @Volatile private var uiResumed = false
    @Volatile private var lastPressAt = Long.MIN_VALUE / 2
    private var registered = false
    private var repost: () -> Unit = {}

    private val dreamReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            dreaming = intent.action == Intent.ACTION_DREAMING_STARTED
        }
    }

    private val lifecycle = object : Application.ActivityLifecycleCallbacks {
        override fun onActivityResumed(activity: Activity) { if (activity is MainActivity) uiResumed = true }
        override fun onActivityPaused(activity: Activity) { if (activity is MainActivity) uiResumed = false }
        override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
        override fun onActivityStarted(activity: Activity) = Unit
        override fun onActivityStopped(activity: Activity) = Unit
        override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
        override fun onActivityDestroyed(activity: Activity) = Unit
    }

    /** Starts tracking the screensaver and the app's own screen. Idempotent. */
    fun register() {
        if (registered) return
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_DREAMING_STARTED)
            addAction(Intent.ACTION_DREAMING_STOPPED)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(dreamReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            context.registerReceiver(dreamReceiver, filter)
        }
        (context.applicationContext as Application).registerActivityLifecycleCallbacks(lifecycle)
        registered = true
    }

    fun unregister() {
        cancel()
        if (!registered) return
        runCatching { context.unregisterReceiver(dreamReceiver) }
        (context.applicationContext as Application).unregisterActivityLifecycleCallbacks(lifecycle)
        registered = false
    }

    /**
     * A call has started ringing. Safe to call from any thread.
     *
     * [repostCallNotification] cancels and posts the incoming-call
     * notification again. Android blocks an app's own background launches for
     * about five seconds after a Home press, but a full-screen intent is
     * launched by the system UI, which is exempt, so re-posting it is how the
     * call screen gets back in front right after Home.
     */
    fun ringStarted(repostCallNotification: () -> Unit) {
        val id = ringId.incrementAndGet()
        handler.post {
            repost = repostCallNotification
            if (CallWakeUpAccessibilityService.isEnabled) {
                step(id, presses = 0, startedAt = SystemClock.elapsedRealtime())
            } else {
                Log.i(TAG, "accessibility service off: plain bring-to-front")
                bringToFront()
            }
        }
    }

    /**
     * True shortly after this guard pressed Home. MainActivity treats a Home
     * press while it is in front as the person leaving and hangs the call up,
     * which must not happen for the app's own press.
     */
    fun pressedHomeRecently(): Boolean = SystemClock.elapsedRealtime() - lastPressAt < OWN_PRESS_WINDOW_MS

    /** The call stopped ringing (answered, declined, missed or cancelled). */
    fun cancel() {
        ringId.incrementAndGet()
    }

    private fun step(id: Int, presses: Int, startedAt: Long) {
        if (id != ringId.get()) return
        if (!power.isInteractive) {
            // Home does nothing while the display is off. If it never comes on, do the old thing.
            if (SystemClock.elapsedRealtime() - startedAt >= SCREEN_ON_TIMEOUT_MS) {
                Log.w(TAG, "screen never came on: plain bring-to-front")
                bringToFront()
                return
            }
            handler.postDelayed({ step(id, presses, startedAt) }, POLL_MS)
            return
        }
        // Home goes first, once; then the app puts itself back in front.
        if (presses == 0) {
            lastPressAt = SystemClock.elapsedRealtime()
            val ok = CallWakeUpAccessibilityService.pressHome()
            Log.i(TAG, "press #1 -> $ok (resumed=$uiResumed dreaming=$dreaming)")
            handler.postDelayed({ if (id == ringId.get()) bringBack() }, BRING_BACK_DELAY_MS)
            handler.postDelayed({ step(id, 1, startedAt) }, VERIFY_DELAY_MS)
            return
        }
        // Checks after the first press. A second Home press on the system's own
        // home screen makes the Portal start the screensaver, so Home is only
        // pressed again when the screensaver really is back, and at most
        // MAX_PRESSES times. Otherwise it just asks to come to the front again.
        when {
            uiResumed && !dreaming -> Log.i(TAG, "settled after $presses press(es)")
            dreaming && presses < MAX_PRESSES -> {
                lastPressAt = SystemClock.elapsedRealtime()
                val ok = CallWakeUpAccessibilityService.pressHome()
                Log.i(TAG, "press #${presses + 1} -> $ok: screensaver is back")
                handler.postDelayed({ if (id == ringId.get()) bringBack() }, BRING_BACK_DELAY_MS)
                handler.postDelayed({ step(id, presses + 1, startedAt) }, VERIFY_DELAY_MS)
            }
            SystemClock.elapsedRealtime() - startedAt < GIVE_UP_MS -> {
                bringBack()
                handler.postDelayed({ step(id, presses, startedAt) }, VERIFY_DELAY_MS)
            }
            else -> Log.w(TAG, "gave up after ${presses} press(es) (resumed=$uiResumed dreaming=$dreaming)")
        }
    }

    private fun bringBack() {
        repost()
        bringToFront()
    }

    private companion object {
        const val TAG = "RingScreenGuard"
        const val POLL_MS = 100L
        const val SCREEN_ON_TIMEOUT_MS = 3_000L
        const val BRING_BACK_DELAY_MS = 150L
        const val VERIFY_DELAY_MS = 700L
        const val GIVE_UP_MS = 8_000L
        const val OWN_PRESS_WINDOW_MS = 2_500L
        const val MAX_PRESSES = 3
    }
}
