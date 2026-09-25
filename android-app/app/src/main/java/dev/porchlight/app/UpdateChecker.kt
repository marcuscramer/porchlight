package dev.porchlight.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageInstaller
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log
import java.io.File
import java.io.IOException
import okhttp3.Call
import okhttp3.Callback
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import org.json.JSONObject

/**
 * Self-updating for a sideload-only app with no Play Store to lean on (see
 * the README's deployment section). Checks a GitHub repo's "latest release"
 * on a schedule, and if it's newer than this build, downloads the APK and
 * posts a notification — tapping it hands the file to Android's own package
 * installer (see [UpdateInstallReceiver]). Deliberately not silent/automatic
 * past that point: this app isn't a system-privileged installer, so Android
 * requires a human to confirm the install regardless of what this class
 * does — the actual goal here is removing the need for someone to show up
 * with a laptop and ADB for routine updates, not a zero-touch pipeline.
 *
 * **Release convention this expects** (follow this wherever releases
 * actually get cut): tag each release `v<versionCode>` — e.g. `v3` for
 * versionCode 3, a plain integer, not semver — and attach the signed APK
 * as a release asset named exactly `app-release.apk`. A release that
 * doesn't match either is silently ignored rather than crashing an update
 * check over a release-process typo.
 *
 * Off by default: [BuildConfig.UPDATE_REPO] is empty until someone sets
 * `porchlight.updateRepo=owner/repo` in the gitignored `local.properties` (or
 * `-PporchlightUpdateRepo=...`) for their own fork/deployment — see
 * `build.gradle.kts`'s own doc. [checkAndMaybeNotify] no-ops entirely on
 * an empty value, so this feature stays inert for anyone who hasn't opted
 * in.
 */
/**
 * Outcome of a single check, for a caller that wants to show something
 * (Settings' "Check for updates" button) rather than just let
 * [UpdateChecker.checkAndMaybeNotify]'s silent notify-or-don't behavior
 * happen in the background.
 */
sealed interface UpdateCheckResult {
    /** [BuildConfig.UPDATE_REPO] is blank — this build has update checking
     * turned off entirely. */
    data object Disabled : UpdateCheckResult
    data object UpToDate : UpdateCheckResult
    // The release's human-readable name ("v0.7"), not its git tag ("v8",
    // the v<versionCode> convention — see this file's own doc). Found
    // live: showing the raw tag here read as a typo/confusing next to
    // Settings' own "v0.7" version line.
    data class Downloading(val versionName: String) : UpdateCheckResult
    data class Ready(val versionName: String) : UpdateCheckResult
    data class Failed(val reason: String) : UpdateCheckResult
}

object UpdateChecker {
    private const val TAG = "UpdateChecker"
    private const val CHANNEL_ID = "portal_call_updates"
    private const val NOTIF_ID = 2
    private const val APK_FILE_NAME = "update.apk"
    private const val RELEASE_ASSET_NAME = "app-release.apk"
    private const val PREFS = "portal_call_updates"
    private const val KEY_NOTIFIED_CODE = "notifiedVersionCode"
    private const val KEY_DOWNLOADED_CODE = "downloadedVersionCode"

    // A real release APK is tens of MB; this is a generous ceiling, not a
    // tight estimate. Guards against buffering an unbounded response fully
    // into memory — a misconfiguration or a compromised repo returning a
    // huge asset would otherwise risk an OutOfMemoryError on Portal's
    // constrained hardware.
    private const val MAX_APK_BYTES = 200L * 1024 * 1024

    // Cheap dedicated client, not shared with NostrSignalingClient's own —
    // this fires at most a couple of times a day and has nothing to do
    // with that class's call/pairing hot path or its OkHttpClient's own
    // lifecycle (see that class's own doc), so sharing would only add
    // coupling for no real benefit.
    private val httpClient = OkHttpClient()

    /**
     * Fetches the latest GitHub release for [BuildConfig.UPDATE_REPO], and
     * if its tag names a versionCode newer than this build's, downloads the
     * APK and posts a notification. Runs entirely on OkHttp's own
     * background dispatcher — safe to call from any thread, including
     * CameraAgentService's startAgent(); deliberately never touches
     * callExecutor (see that field's own "Threading" doc) — this has
     * nothing to do with call/pairing state, and a multi-MB download has
     * no business blocking that single-threaded executor even briefly.
     * Every failure (network, parse, mismatched release-naming convention)
     * is logged and swallowed — a failed update check is never worth
     * surfacing to whoever's watching TV, and the next scheduled check
     * just tries again.
     */
    fun checkAndMaybeNotify(context: Context) = checkNow(context, force = false) {}

    /**
     * The real check, shared by the silent scheduled path
     * ([checkAndMaybeNotify], `force = false`, result ignored) and
     * [AdminChoiceScreen]'s manual "Check for updates" button (`force =
     * true`) that wants to show [UpdateCheckResult] rather than let it
     * happen invisibly. [onResult] can fire more than once for one call
     * (e.g. [UpdateCheckResult.Downloading] then [UpdateCheckResult.Ready])
     * and always fires on whatever background thread OkHttp's callback
     * runs on, same as [checkAndMaybeNotify] always has — callers touching
     * UI state need to hop back to the main thread themselves.
     *
     * `force`: the scheduled path's own dedup (never re-notify for a
     * version already notified once) would otherwise make a manual re-check
     * silently do nothing the moment a background check already found the
     * same release — exactly the case someone tapping "Check for updates"
     * most wants a real answer for, not silence.
     */
    fun checkNow(context: Context, force: Boolean, onResult: (UpdateCheckResult) -> Unit) {
        val repo = BuildConfig.UPDATE_REPO
        if (repo.isBlank()) { onResult(UpdateCheckResult.Disabled); return }
        val appContext = context.applicationContext
        val request = Request.Builder()
            .url("https://api.github.com/repos/$repo/releases/latest")
            .header("Accept", "application/vnd.github+json")
            .build()
        httpClient.newCall(request).enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) {
                Log.w(TAG, "update check failed", e)
                onResult(UpdateCheckResult.Failed("network"))
            }
            override fun onResponse(call: Call, response: Response) {
                response.use {
                    if (!it.isSuccessful) {
                        Log.w(TAG, "update check: unexpected response ${it.code}")
                        onResult(UpdateCheckResult.Failed("server (${it.code})"))
                        return
                    }
                    handleReleaseResponse(appContext, it.body.string(), force, onResult)
                }
            }
        })
    }

    private fun handleReleaseResponse(context: Context, body: String, force: Boolean, onResult: (UpdateCheckResult) -> Unit) {
        val release = runCatching { JSONObject(body) }.getOrNull()
        if (release == null) {
            onResult(UpdateCheckResult.Failed("couldn't parse response"))
            return
        }
        val tag = release.optString("tag_name")
        // The release's own title ("v0.7") — what every display/notification
        // site below shows, as opposed to [tag] (only ever used here, to
        // parse latestCode against the v<versionCode> convention). Falls
        // back to the tag itself if a release was ever published with no
        // name set.
        val versionName = release.optString("name").ifBlank { tag }
        // startsWith("v") checked explicitly, not just removePrefix: a tag
        // with no "v" at all would otherwise still parse (removePrefix is a
        // no-op when the prefix isn't present) — looser than the documented
        // v<versionCode> convention.
        val latestCode = if (tag.startsWith("v")) tag.removePrefix("v").toIntOrNull() else null
        if (latestCode == null) {
            Log.w(TAG, "release tag '$tag' doesn't match the v<versionCode> convention; ignoring")
            onResult(UpdateCheckResult.Failed("unrecognized release"))
            return
        }
        if (latestCode <= BuildConfig.VERSION_CODE) {
            onResult(UpdateCheckResult.UpToDate)
            return
        }

        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

        // Already downloaded this exact version and the file's still on
        // disk — report it as ready without re-fetching the multi-MB APK.
        // Found live: Settings calls this with force = true on every visit
        // (see AdminChoiceScreen's own doc), and force only bypasses the
        // notify-dedup below, so without this check re-opening Settings
        // re-downloaded the same APK every single time. apkFile() lives in
        // cacheDir, which Android can reclaim under storage pressure, so
        // this also checks the file actually still exists rather than
        // trusting the pref alone.
        if (prefs.getInt(KEY_DOWNLOADED_CODE, 0) == latestCode && apkFile(context).exists()) {
            onResult(UpdateCheckResult.Ready(versionName))
            return
        }

        if (!force && prefs.getInt(KEY_NOTIFIED_CODE, 0) == latestCode) return // already notified for this exact version

        val assets = release.optJSONArray("assets") ?: run {
            onResult(UpdateCheckResult.Failed("malformed release"))
            return
        }
        var downloadUrl: String? = null
        for (i in 0 until assets.length()) {
            val asset = assets.optJSONObject(i) ?: continue
            if (asset.optString("name") == RELEASE_ASSET_NAME) {
                downloadUrl = asset.optString("browser_download_url").ifBlank { null }
                break
            }
        }
        if (downloadUrl == null) {
            Log.w(TAG, "release $tag has no $RELEASE_ASSET_NAME asset; ignoring")
            onResult(UpdateCheckResult.Failed("release has no APK attached"))
            return
        }

        onResult(UpdateCheckResult.Downloading(versionName))
        downloadApk(context, downloadUrl) { success ->
            if (success) {
                prefs.edit()
                    .putInt(KEY_NOTIFIED_CODE, latestCode)
                    .putInt(KEY_DOWNLOADED_CODE, latestCode)
                    .apply()
                postUpdateNotification(context, versionName)
                onResult(UpdateCheckResult.Ready(versionName))
            } else {
                onResult(UpdateCheckResult.Failed("download failed"))
            }
        }
    }

    private fun downloadApk(context: Context, url: String, onDone: (Boolean) -> Unit) {
        val request = Request.Builder().url(url).build()
        httpClient.newCall(request).enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) {
                Log.w(TAG, "update download failed", e)
                onDone(false)
            }
            override fun onResponse(call: Call, response: Response) {
                response.use {
                    if (!it.isSuccessful) {
                        Log.w(TAG, "update download: unexpected response ${it.code}")
                        onDone(false)
                        return
                    }
                    val declaredLength = it.body.contentLength()
                    if (declaredLength > MAX_APK_BYTES) {
                        Log.e(TAG, "update download: declared size $declaredLength exceeds ${MAX_APK_BYTES}-byte cap; refusing")
                        onDone(false)
                        return
                    }
                    // Streamed to disk with a running cap, not
                    // it.body.bytes() (buffers the entire response into
                    // memory first) — Content-Length can be absent/wrong, so
                    // the cap has to be enforced on actual bytes read too.
                    val written = runCatching {
                        apkFile(context).outputStream().use { out ->
                            it.body.byteStream().use { input ->
                                val buffer = ByteArray(8192)
                                var total = 0L
                                while (true) {
                                    val read = input.read(buffer)
                                    if (read == -1) break
                                    total += read
                                    if (total > MAX_APK_BYTES) error("update download exceeded ${MAX_APK_BYTES}-byte cap")
                                    out.write(buffer, 0, read)
                                }
                            }
                        }
                    }
                    if (written.isFailure) {
                        Log.e(TAG, "failed to write downloaded APK", written.exceptionOrNull())
                        apkFile(context).delete()
                        onDone(false)
                        return
                    }
                    onDone(true)
                }
            }
        })
    }

    private fun apkFile(context: Context): File = File(context.cacheDir, APK_FILE_NAME)

    private fun postUpdateNotification(context: Context, versionName: String) {
        val mgr = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (mgr.getNotificationChannel(CHANNEL_ID) == null) {
            mgr.createNotificationChannel(
                NotificationChannel(CHANNEL_ID, "Updates", NotificationManager.IMPORTANCE_DEFAULT)
                    .apply { description = "A newer Porchlight build is ready to install." },
            )
        }
        // A BroadcastReceiver, not a direct install Intent: whether tapping
        // this can go straight to the installer or has to detour through
        // Settings first (see UpdateInstallReceiver's own doc) depends on
        // a permission state that can change between when this notification
        // is posted and when it's actually tapped, so that decision has to
        // be made fresh at tap time, not baked in now.
        val tapIntent = Intent(context, UpdateInstallReceiver::class.java)
        val pendingIntent = PendingIntent.getBroadcast(
            context, 0, tapIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = Notification.Builder(context, CHANNEL_ID)
            .setContentTitle("Porchlight update ready")
            .setContentText("$versionName downloaded — tap to install")
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentIntent(pendingIntent)
            .setAutoCancel(true)
            .build()
        mgr.notify(NOTIF_ID, notification)
    }

    /**
     * Installs the already-downloaded update APK if this app currently has
     * install permission, otherwise sends the user to the one Settings
     * screen that grants it. Shared by [UpdateInstallReceiver] (a tap on
     * the "update ready" notification) and Settings' own in-app "Install
     * now" button — the notification alone isn't a reliable path on
     * Android TV: confirmed live that a correctly-posted notification
     * never surfaces in the Google TV notification panel at all, leaving
     * no way to reach it, so the in-app button is the path that actually
     * works on this platform.
     */
    fun installOrRequestPermission(context: Context) {
        if (context.packageManager.canRequestPackageInstalls()) {
            launchInstall(context)
        } else {
            context.startActivity(
                Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${context.packageName}"))
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }

    private const val INSTALL_RESULT_ACTION = "dev.porchlight.app.UPDATE_INSTALL_RESULT"

    /**
     * Launches the system package installer for the already-downloaded
     * APK — called once install permission is confirmed. Separated out so
     * both [installOrRequestPermission] and any future caller share one
     * path.
     *
     * A real [PackageInstaller] session, not a plain `ACTION_VIEW` intent
     * handing the APK to whatever resolves it (the previous approach,
     * which needed [FileProvider] to share a `content://` URI across that
     * app boundary — no longer needed, since a session reads the file
     * itself). Found live on real Portal TV hardware: `ACTION_VIEW`
     * resolves to `com.android.packageinstaller`'s standard UI flow,
     * which on Portal is intercepted by a Meta-proprietary
     * `FacebookAppVerifier` system service that checks the APK's signing
     * certificate against a whitelist of Meta's own internal keys and
     * unconditionally rejects anything else ("App certificate rejected",
     * confirmed directly in logcat) — nothing an app can do about that
     * from user space. `adb install` was never affected (it never goes
     * through that UI flow at all), which is why every manual install
     * this project has ever done worked fine while this in-app button
     * silently failed. Immortal's own self-update/app-store install path
     * (`PackageInstallSessions.kt`/`HeadlessInstaller.kt` in its own
     * repo) uses this exact same session API for the identical reason —
     * confirmed against its real source, not guessed.
     */
    fun launchInstall(context: Context) {
        val file = apkFile(context)
        if (!file.exists()) return
        val appContext = context.applicationContext
        val installer = appContext.packageManager.packageInstaller
        val sessionId = runCatching {
            installer.createSession(PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL))
        }.getOrElse {
            Log.e(TAG, "couldn't create install session", it)
            return
        }

        // Registered fresh per install attempt, not a persistent
        // manifest-declared receiver — this only ever needs to react to
        // the one PendingIntent [session.commit] below creates for this
        // specific sessionId, and unregisters itself once that session
        // reaches a terminal state (or hands off to the system's own
        // confirmation UI, which the human taps same as before).
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(receiverContext: Context, intent: Intent) {
                when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, Int.MIN_VALUE)) {
                    PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                        val confirm = if (Build.VERSION.SDK_INT >= 33) {
                            intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                        } else {
                            @Suppress("DEPRECATION") intent.getParcelableExtra(Intent.EXTRA_INTENT)
                        }
                        confirm?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                        runCatching { appContext.startActivity(confirm) }
                            .onFailure { Log.w(TAG, "couldn't launch install confirmation UI", it) }
                    }
                    else -> runCatching { appContext.unregisterReceiver(this) }
                }
            }
        }
        val filter = IntentFilter(INSTALL_RESULT_ACTION)
        if (Build.VERSION.SDK_INT >= 33) {
            appContext.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            appContext.registerReceiver(receiver, filter)
        }

        runCatching {
            installer.openSession(sessionId).use { session ->
                session.openWrite("base.apk", 0, file.length()).use { out ->
                    file.inputStream().use { it.copyTo(out) }
                    session.fsync(out)
                }
                val flags = if (Build.VERSION.SDK_INT >= 31) {
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE
                } else {
                    PendingIntent.FLAG_UPDATE_CURRENT
                }
                val resultIntent = Intent(INSTALL_RESULT_ACTION).setPackage(appContext.packageName)
                val pending = PendingIntent.getBroadcast(appContext, sessionId, resultIntent, flags)
                session.commit(pending.intentSender)
            }
        }.onFailure {
            Log.e(TAG, "install session failed", it)
            runCatching { appContext.unregisterReceiver(receiver) }
        }
    }
}
