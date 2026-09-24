package dev.porchlight.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Two separate things after a reboot, not one:
 *
 * The background call-answering service always restarts, unconditionally
 * — silent, nothing shows on screen, and losing the ability to receive
 * calls after a power blip until a human notices and manually reopens the
 * app would be a real regression for every install. `CameraAgentService.
 * start` self-checks `Config.isValid` and stops itself on an unconfigured
 * device, so calling it unconditionally here is safe even before any
 * pairing exists.
 *
 * Forcing this app's own UI to the foreground, on top of whatever else
 * was showing (Immortal's home, another app), is a different matter — an
 * opt-in choice (`Config.launchOnBoot`, off by default) for a device set
 * up as a single-purpose calling appliance, not something every install
 * should get unasked.
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        CameraAgentService.start(context)
        if (Config.load(context).launchOnBoot) {
            context.startActivity(
                Intent(context, MainActivity::class.java).setFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
        }
    }
}
