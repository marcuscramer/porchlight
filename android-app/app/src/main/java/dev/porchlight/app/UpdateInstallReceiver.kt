package dev.porchlight.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings

/**
 * Handles a tap on the "update ready" notification (see [UpdateChecker]).
 * Routes to one of two places depending on whether this app currently has
 * install permission: straight to the package installer, or — the first
 * time, on any given device — to the one Settings screen a human has to
 * flip manually, since Android doesn't let [REQUEST_INSTALL_PACKAGES]
 * grant itself the way a runtime permission dialog can.
 */
class UpdateInstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (context.packageManager.canRequestPackageInstalls()) {
            UpdateChecker.launchInstall(context)
        } else {
            context.startActivity(
                Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${context.packageName}"))
                    .setFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }
}
