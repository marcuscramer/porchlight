package dev.porchlight.app

import android.accessibilityservice.AccessibilityService
import android.util.Log
import android.view.accessibility.AccessibilityEvent
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * Presses the Home key on request, and does nothing else. A short press of
 * Home makes the Portal send HDMI-CEC `<Text View On>` and `<Active Source>`,
 * which switches the TV to the Portal's input; no other app-reachable action
 * does (see the HDMI experiment notes). The `HDMI_CEC` permission needed to
 * send those messages directly is signature-level, so the app can't, but the
 * system's own key handling can.
 *
 * Declared with no event types and no content access (see
 * res/xml/call_wake_up_accessibility_service.xml), so it can't read the screen.
 * It's off until the person turns it on in the system's Accessibility settings.
 */
class CallWakeUpAccessibilityService : AccessibilityService() {
    override fun onAccessibilityEvent(event: AccessibilityEvent?) = Unit

    override fun onInterrupt() = Unit

    override fun onServiceConnected() {
        super.onServiceConnected()
        instance = this
        _enabled.value = true
        Log.i(TAG, "connected")
    }

    override fun onUnbind(intent: android.content.Intent?): Boolean {
        instance = null
        _enabled.value = false
        return super.onUnbind(intent)
    }

    companion object {
        private const val TAG = "CallWakeUpA11y"

        @Volatile
        var instance: CallWakeUpAccessibilityService? = null
            private set

        private val _enabled = MutableStateFlow(false)

        /** True while the person has the service switched on, observable by the UI. */
        val enabled: StateFlow<Boolean> = _enabled

        /** True while the person has the service switched on. */
        val isEnabled: Boolean get() = instance != null

        /** Presses Home; false if the service isn't on or the system refused. */
        fun pressHome(): Boolean = instance?.performGlobalAction(GLOBAL_ACTION_HOME) ?: false
    }
}
