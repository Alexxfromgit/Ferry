package com.xenvaultinteractive.ferry

import android.app.Activity
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.core.content.FileProvider
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import org.json.JSONObject
import java.util.concurrent.Executors

/**
 * Updates for the Direct build (sideloaded from GitHub or ferry.xenvaultinteractive.com).
 *
 * The feed at [FEED_URL] names the latest build. A newer one is downloaded into the cache and
 * only offered once four checks pass: the feed is well formed and points at the same host, the
 * file has exactly the announced size and SHA-256, and the APK is this app, with the announced
 * version code, signed by the same certificates as the installed copy. Android's installer then
 * asks the user to confirm.
 */
@TauriPlugin
class UpdatePlugin(private val activity: Activity) : Plugin(activity) {
    private val io = Executors.newSingleThreadExecutor()
    private val dir get() = File(activity.cacheDir, "direct-updates")

    @InvokeArg
    class CheckArgs {
        var automatic: Boolean = false
    }

    /** The version of a verified update waiting to be installed, if any. */
    @Command
    fun ready(invoke: Invoke) {
        io.execute {
            invoke.resolve(JSObject().apply { put("readyVersion", readyUpdate()?.versionName) })
        }
    }

    /** Look for a newer build; download and verify it. Resolves with its version, or null. */
    @Command
    fun check(invoke: Invoke) {
        io.execute {
            try {
                invoke.resolve(JSObject().apply { put("readyVersion", checkAndDownload()) })
            } catch (e: Exception) {
                invoke.reject(e.message ?: "Couldn't check for updates")
            }
        }
    }

    /** Hand the verified update to Android's installer. */
    @Command
    fun install(invoke: Invoke) {
        io.execute {
            val update = try {
                readyUpdate() ?: throw IOException("No update is ready to install.")
            } catch (e: Exception) {
                invoke.reject(e.message ?: "No update is ready to install.")
                return@execute
            }
            activity.runOnUiThread {
                try {
                    if (!activity.packageManager.canRequestPackageInstalls()) {
                        activity.startActivity(
                            Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${activity.packageName}"))
                        )
                        invoke.reject("Allow Ferry Direct to install apps in the screen that opened, then choose Install update again.")
                        return@runOnUiThread
                    }
                    val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", update.file)
                    activity.startActivity(
                        Intent(Intent.ACTION_VIEW)
                            .setDataAndType(uri, "application/vnd.android.package-archive")
                            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    )
                    invoke.resolve()
                } catch (e: Exception) {
                    invoke.reject(e.message ?: "Couldn't start the installer")
                }
            }
        }
    }

    private class Feed(
        val versionCode: Long,
        val versionName: String,
        val apkUrl: URL,
        val sha256: String,
        val sizeBytes: Long,
    )

    private class Ready(val file: File, val versionName: String)

    private fun installedVersionCode(): Long =
        activity.packageManager.getPackageInfo(activity.packageName, 0).longVersionCode

    private fun checkAndDownload(): String? {
        val feed = fetchFeed()
        if (feed.versionCode <= installedVersionCode()) {
            dir.deleteRecursively()
            return null
        }
        val apk = File(dir, "ferry-${feed.versionCode}.apk")
        val meta = File(dir, "ferry-${feed.versionCode}.json")
        if (!(apk.isFile && verifyArchive(apk, feed.versionCode) && sha256Of(apk) == feed.sha256)) {
            dir.deleteRecursively()
            dir.mkdirs()
            download(feed, apk)
            if (!verifyArchive(apk, feed.versionCode)) {
                apk.delete()
                throw IOException("The downloaded update isn't signed like this copy of Ferry, so it was discarded.")
            }
        }
        meta.writeText(JSONObject().put("versionCode", feed.versionCode).put("versionName", feed.versionName).toString())
        return feed.versionName
    }

    private fun readyUpdate(): Ready? {
        val files = dir.listFiles { f -> f.name.endsWith(".json") } ?: return null
        for (meta in files) {
            val json = runCatching { JSONObject(meta.readText()) }.getOrNull() ?: continue
            val versionCode = json.optLong("versionCode")
            val apk = File(dir, "ferry-$versionCode.apk")
            if (versionCode > installedVersionCode() && apk.isFile && verifyArchive(apk, versionCode)) {
                return Ready(apk, json.optString("versionName"))
            }
        }
        return null
    }

    private fun fetchFeed(): Feed {
        val json = JSONObject(String(get(URL(FEED_URL)).use { it.readBounded(MAX_FEED_BYTES) }, Charsets.UTF_8))
        val feed = Feed(
            versionCode = json.getLong("versionCode"),
            versionName = json.getString("versionName"),
            apkUrl = URL(json.getString("apkUrl")),
            sha256 = json.getString("sha256").lowercase(),
            sizeBytes = json.getLong("sizeBytes"),
        )
        val valid = feed.versionCode > 0 &&
            feed.versionName.isNotBlank() &&
            feed.apkUrl.protocol == "https" &&
            feed.apkUrl.host == FEED_HOST &&
            feed.apkUrl.path.endsWith(".apk") &&
            feed.sha256.matches(Regex("[0-9a-f]{64}")) &&
            feed.sizeBytes in 1..MAX_APK_BYTES
        if (!valid) throw IOException("The update feed is malformed.")
        return feed
    }

    private fun download(feed: Feed, target: File) {
        val part = File(target.path + ".part")
        val digest = MessageDigest.getInstance("SHA-256")
        var total = 0L
        get(feed.apkUrl).use { input ->
            part.outputStream().use { out ->
                val buffer = ByteArray(1 shl 16)
                while (true) {
                    val n = input.read(buffer)
                    if (n < 0) break
                    total += n
                    if (total > feed.sizeBytes) throw IOException("The update is larger than announced.")
                    digest.update(buffer, 0, n)
                    out.write(buffer, 0, n)
                }
            }
        }
        if (total != feed.sizeBytes) throw IOException("The update download was incomplete.")
        if (hex(digest.digest()) != feed.sha256) {
            part.delete()
            throw IOException("The update didn't match its checksum, so it was discarded.")
        }
        if (!part.renameTo(target)) throw IOException("Couldn't store the update.")
    }

    /** The APK is this app, at [versionCode], signed by exactly the installed certificates. */
    private fun verifyArchive(apk: File, versionCode: Long): Boolean {
        val pm = activity.packageManager
        val flags = PackageManager.GET_SIGNING_CERTIFICATES
        val archive = pm.getPackageArchiveInfo(apk.path, flags) ?: return false
        val installed = pm.getPackageInfo(activity.packageName, flags)
        return archive.packageName == activity.packageName &&
            archive.longVersionCode == versionCode &&
            signers(archive).isNotEmpty() &&
            signers(archive) == signers(installed)
    }

    private fun signers(info: PackageInfo): Set<String> {
        val signing = info.signingInfo ?: return emptySet()
        val certs = if (signing.hasMultipleSigners()) signing.apkContentsSigners else signing.signingCertificateHistory
        return certs.orEmpty().map { hex(MessageDigest.getInstance("SHA-256").digest(it.toByteArray())) }.toSet()
    }

    /** HTTPS GET from the update host only, with no redirects. */
    private fun get(url: URL): InputStream {
        if (url.protocol != "https" || url.host != FEED_HOST) throw IOException("Refusing to download from ${url.host}.")
        val connection = url.openConnection() as HttpURLConnection
        connection.instanceFollowRedirects = false
        connection.connectTimeout = 15_000
        connection.readTimeout = 30_000
        connection.setRequestProperty("Cache-Control", "no-cache")
        if (connection.responseCode != 200) {
            connection.disconnect()
            throw IOException("The update server answered ${connection.responseCode}.")
        }
        return connection.inputStream
    }

    private fun InputStream.readBounded(limit: Int): ByteArray {
        val bytes = readBytes()
        if (bytes.size > limit) throw IOException("The update feed is too large.")
        return bytes
    }

    private fun sha256Of(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(1 shl 16)
            while (true) {
                val n = input.read(buffer)
                if (n < 0) break
                digest.update(buffer, 0, n)
            }
        }
        return hex(digest.digest())
    }

    private fun hex(bytes: ByteArray) = bytes.joinToString("") { "%02x".format(it) }

    companion object {
        private const val FEED_HOST = "downloads.xenvaultinteractive.com"
        private const val FEED_URL = "https://$FEED_HOST/ferry/android/latest.json"
        private const val MAX_FEED_BYTES = 64 * 1024
        private const val MAX_APK_BYTES = 300L * 1024 * 1024
    }
}
