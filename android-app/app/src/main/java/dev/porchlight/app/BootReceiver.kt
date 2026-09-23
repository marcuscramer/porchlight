package dev.porchlight.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Launches straight back into the call screen after a reboot — nobody
 * should have to touch a remote for the device to come back to life. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        context.startActivity(
            Intent(context, MainActivity::class.java).setFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        )
    }
}
