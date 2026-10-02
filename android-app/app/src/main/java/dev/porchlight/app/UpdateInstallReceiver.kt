package dev.porchlight.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Handles a tap on the "update ready" notification (see [UpdateChecker]).
 * Kept as a fallback for platforms where that notification is actually
 * reachable — Settings' own "Install now" button ([UpdateChecker.installOrRequestPermission])
 * is the path that works on Android TV.
 */
class UpdateInstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        UpdateChecker.installOrRequestPermission(context)
    }
}
