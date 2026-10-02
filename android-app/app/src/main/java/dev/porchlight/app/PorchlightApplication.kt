package dev.porchlight.app

import android.app.AlarmManager
import android.app.Application
import android.app.PendingIntent
import android.content.Intent
import android.os.Process
import android.util.Log
import kotlin.system.exitProcess
import org.webrtc.PeerConnectionFactory

/**
 * Nobody debugging a frozen TV — an uncaught exception anywhere in the app
 * schedules a relaunch a second out, then kills the crashing process
 * cleanly so Android never shows an "app has stopped" dialog that would
 * just sit there forever waiting for someone to dismiss it.
 */
class PorchlightApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        // Registers the JNI bindings the whole org.webrtc package depends
        // on — without this, using camera/native WebRTC classes crashes with
        // `UnsatisfiedLinkError: no implementation found for
        // Histogram.nativeCreateCounts`. `initialize()` alone isn't enough:
        // the actual native method registration only happens the first time
        // a PeerConnectionFactory is built, so build one here and dispose it
        // immediately — the registration is process-global and outlives
        // this instance, so nothing is lost, and WebRtcEngine.start() still
        // builds the real one later, per call. A factory alone doesn't
        // acquire the camera/mic — only creating a video/audio source from
        // it does — so this is safe at startup, before a call exists.
        PeerConnectionFactory.initialize(
            PeerConnectionFactory.InitializationOptions.builder(this).createInitializationOptions()
        )
        PeerConnectionFactory.builder().createPeerConnectionFactory().dispose()
        Thread.setDefaultUncaughtExceptionHandler { _, throwable ->
            Log.e(TAG, "uncaught exception — restarting", throwable)
            try {
                val restart = Intent(this, MainActivity::class.java)
                    .setFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
                val pendingIntent = PendingIntent.getActivity(
                    this, 0, restart, PendingIntent.FLAG_IMMUTABLE,
                )
                val alarmManager = getSystemService(ALARM_SERVICE) as AlarmManager
                alarmManager.setExactAndAllowWhileIdle(
                    AlarmManager.RTC_WAKEUP, System.currentTimeMillis() + 1000, pendingIntent,
                )
            } catch (t: Throwable) {
                Log.e(TAG, "failed to schedule restart", t)
            }
            Process.killProcess(Process.myPid())
            exitProcess(10)
        }
    }

    companion object {
        private const val TAG = "PorchlightApp"
    }
}
