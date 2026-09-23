package dev.porchlight.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.core.content.FileProvider
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
object UpdateChecker {
    private const val TAG = "UpdateChecker"
    private const val CHANNEL_ID = "portal_call_updates"
    private const val NOTIF_ID = 2
    private const val APK_FILE_NAME = "update.apk"
    private const val RELEASE_ASSET_NAME = "app-release.apk"
    private const val PREFS = "portal_call_updates"
    private const val KEY_NOTIFIED_CODE = "notifiedVersionCode"

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
    fun checkAndMaybeNotify(context: Context) {
        val repo = BuildConfig.UPDATE_REPO
        if (repo.isBlank()) return
        val appContext = context.applicationContext
        val request = Request.Builder()
            .url("https://api.github.com/repos/$repo/releases/latest")
            .header("Accept", "application/vnd.github+json")
            .build()
        httpClient.newCall(request).enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) {
                Log.w(TAG, "update check failed", e)
            }
            override fun onResponse(call: Call, response: Response) {
                response.use {
                    if (!it.isSuccessful) {
                        Log.w(TAG, "update check: unexpected response ${it.code}")
                        return
                    }
                    handleReleaseResponse(appContext, it.body.string())
                }
            }
        })
    }

    private fun handleReleaseResponse(context: Context, body: String) {
        val release = runCatching { JSONObject(body) }.getOrNull() ?: return
        val tag = release.optString("tag_name")
        // startsWith("v") checked explicitly, not just removePrefix: a tag
        // with no "v" at all would otherwise still parse (removePrefix is a
        // no-op when the prefix isn't present) — looser than the documented
        // v<versionCode> convention.
        val latestCode = if (tag.startsWith("v")) tag.removePrefix("v").toIntOrNull() else null
        if (latestCode == null) {
            Log.w(TAG, "release tag '$tag' doesn't match the v<versionCode> convention; ignoring")
            return
        }
        if (latestCode <= BuildConfig.VERSION_CODE) return // already up to date

        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        if (prefs.getInt(KEY_NOTIFIED_CODE, 0) == latestCode) return // already notified for this exact version

        val assets = release.optJSONArray("assets") ?: return
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
            return
        }

        downloadApk(context, downloadUrl) { success ->
            if (success) {
                prefs.edit().putInt(KEY_NOTIFIED_CODE, latestCode).apply()
                postUpdateNotification(context, tag)
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

    private fun postUpdateNotification(context: Context, tag: String) {
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
            .setContentText("$tag downloaded — tap to install")
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentIntent(pendingIntent)
            .setAutoCancel(true)
            .build()
        mgr.notify(NOTIF_ID, notification)
    }

    /**
     * Launches the system package installer for the already-downloaded
     * APK — called from [UpdateInstallReceiver] once install permission is
     * confirmed. Separated out so both it and any future caller (e.g. a
     * manual "Install now" action) share one path.
     */
    fun launchInstall(context: Context) {
        val file = apkFile(context)
        if (!file.exists()) return
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.fileprovider", file)
        val intent = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_GRANT_READ_URI_PERMISSION)
        context.startActivity(intent)
    }
}
