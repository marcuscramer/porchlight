package dev.porchlight.app

import android.app.Activity
import android.app.Application
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.PixelFormat
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.PowerManager
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import android.view.View
import android.view.WindowManager
import androidx.compose.ui.graphics.toArgb
import dev.porchlight.app.ui.theme.GeneratedColor
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
 * resumed and the screensaver is not running, pressing Home again only if
 * the screensaver is back. All of those decisions, and every timing, live
 * in the core's `wake_up` module (`call-core/src/wake_up.rs`), where they
 * are covered by `cargo test`; this class only observes the device and
 * carries each step out. Without the accessibility service enabled (or with
 * the Settings switch off) it does a plain bring-to-front.
 *
 * Pressing Home briefly shows whatever Android resolves Home to (normally
 * the Portal's own home screen) before this app gets back in front — around
 * the Portal's own one-touch-play response time, too fast to avoid, just to
 * see. If the "display over other apps" permission is also granted, a plain
 * navy panel covers the screen for that moment instead: added right before
 * the first press, removed once settled (or on give-up, which already shows
 * the app's content). A timer force-removes it regardless, so a bug here can
 * never leave the screen covered.
 */
internal class RingScreenGuard(
    private val context: Context,
    private val handler: Handler,
    private val bringToFront: () -> Unit,
) {
    private val power = context.getSystemService(Context.POWER_SERVICE) as PowerManager
    private val windowManager = context.getSystemService(Context.WINDOW_SERVICE) as WindowManager

    // Bumped on every start and cancel; a step scheduled for an older ring
    // sees a different value and does nothing.
    private val ringId = AtomicInteger(0)

    @Volatile private var dreaming = false
    @Volatile private var uiResumed = false
    @Volatile private var lastPressAt = Long.MIN_VALUE / 2
    private var registered = false
    private var repost: () -> Unit = {}
    private var maskView: View? = null
    private val hideMaskAfterTimeout = Runnable { hideMask() }

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
            // Already cancelled (the caller hung up within a few ms): don't
            // show a mask nothing is left to remove — cancel() could only
            // hide one that existed when it ran, and this block runs later.
            if (id != ringId.get()) return@post
            repost = repostCallNotification
            if (CallCoreBridge.wakeUpShouldEscalate(CallWakeUpAccessibilityService.isEnabled, Config.load(context).callWakeUp)) {
                showMask()
                step(id, presses = 0, startedAt = SystemClock.elapsedRealtime())
            } else {
                Log.i(TAG, "call wake-up off (service not enabled, or switched off in Settings): plain bring-to-front")
                bringToFront()
            }
        }
    }

    /**
     * True shortly after this guard pressed Home. MainActivity treats a Home
     * press while it is in front as the person leaving and hangs the call up,
     * which must not happen for the app's own press.
     */
    fun pressedHomeRecently(): Boolean = SystemClock.elapsedRealtime() - lastPressAt < CallCoreBridge.wakeUpConstants.ownPressWindowMs

    /** The call stopped ringing (answered, declined, missed or cancelled). */
    fun cancel() {
        ringId.incrementAndGet()
        // The mask is a window, so it can only be removed from the main
        // thread; cancel() is called from the call executor.
        handler.post { hideMask() }
    }

    /**
     * One step of the wake-up. *What* to do next (wait for the display, press
     * Home, bring the app back, settle, give up) is decided by the core's
     * `wake_up` module from what is observed here; this only looks at the
     * device and carries the decision out. A step belonging to a ring that
     * has since been cancelled does nothing.
     */
    private fun step(id: Int, presses: Int, startedAt: Long) {
        if (id != ringId.get()) return
        val elapsed = SystemClock.elapsedRealtime() - startedAt
        when (val next = CallCoreBridge.wakeUpNextStep(elapsed, presses, power.isInteractive, uiResumed, dreaming)) {
            is CallCoreBridge.WakeUpStep.WaitForScreen ->
                handler.postDelayed({ step(id, presses, startedAt) }, next.recheckAfterMs)
            CallCoreBridge.WakeUpStep.PlainBringToFront -> {
                Log.w(TAG, "screen never came on: plain bring-to-front")
                bringToFront()
            }
            is CallCoreBridge.WakeUpStep.PressHome -> {
                lastPressAt = SystemClock.elapsedRealtime()
                val ok = CallWakeUpAccessibilityService.pressHome()
                Log.i(TAG, "press #${next.pressNumber} -> $ok (resumed=$uiResumed dreaming=$dreaming)")
                handler.postDelayed({ if (id == ringId.get()) bringBack() }, next.bringBackAfterMs)
                handler.postDelayed({ step(id, next.pressNumber, startedAt) }, next.verifyAfterMs)
            }
            is CallCoreBridge.WakeUpStep.BringBack -> {
                bringBack()
                handler.postDelayed({ step(id, presses, startedAt) }, next.recheckAfterMs)
            }
            CallCoreBridge.WakeUpStep.Settled -> {
                Log.i(TAG, "settled after $presses press(es)")
                hideMask()
            }
            CallCoreBridge.WakeUpStep.GiveUp -> {
                Log.w(TAG, "gave up after $presses press(es) (resumed=$uiResumed dreaming=$dreaming)")
                hideMask()
            }
        }
    }

    private fun bringBack() {
        repost()
        bringToFront()
    }

    /**
     * A plain opaque panel in the screens' own navy, covering whatever Home
     * resolves to until this side is confirmed back in front. Only shown if
     * "display over other apps" is granted (optional, alongside the
     * accessibility service — see README); a no-op otherwise. Deliberately
     * doesn't cover the screensaver itself (a different window layer a plain
     * overlay can't reach) — not needed, since Home while the screensaver is
     * running ends it directly without ever showing a home screen.
     */
    private fun showMask() {
        if (maskView != null) return
        if (!Settings.canDrawOverlays(context)) return
        val view = View(context).apply { setBackgroundColor(GeneratedColor.colorBackgroundWaiting.toArgb()) }
        val type = WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
        val params = WindowManager.LayoutParams(
            WindowManager.LayoutParams.MATCH_PARENT,
            WindowManager.LayoutParams.MATCH_PARENT,
            type,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE,
            PixelFormat.OPAQUE,
        )
        runCatching {
            windowManager.addView(view, params)
            maskView = view
            handler.postDelayed(hideMaskAfterTimeout, CallCoreBridge.wakeUpConstants.maskTimeoutMs)
        }.onFailure { Log.e(TAG, "showMask failed", it) }
    }

    private fun hideMask() {
        handler.removeCallbacks(hideMaskAfterTimeout)
        val view = maskView ?: return
        maskView = null
        runCatching { windowManager.removeView(view) }.onFailure { Log.e(TAG, "hideMask failed", it) }
    }

    private companion object {
        const val TAG = "RingScreenGuard"
    }
}
