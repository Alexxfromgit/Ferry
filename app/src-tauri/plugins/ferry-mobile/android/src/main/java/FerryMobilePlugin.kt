package com.xenvaultinteractive.ferry.mobile

import android.Manifest
import android.app.Activity
import android.app.DownloadManager
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ContentValues
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.core.content.IntentCompat
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.FileInputStream
import java.io.IOException
import java.util.concurrent.Executors

@InvokeArg
class SaveArgs {
    lateinit var path: String
    lateinit var relativeDir: String
}

@InvokeArg
class UriArgs {
    lateinit var uri: String
}

@InvokeArg
class UrisArgs {
    lateinit var uris: Array<String>
}

@InvokeArg
class TransferArgs {
    var active: Boolean = false
    var title: String = ""
    var text: String = ""
    var progress: Int? = null
    /** Shares are on: add a "Stop sharing" button, and no progress bar while nothing moves. */
    var sharing: Boolean = false
}

@InvokeArg
class WatchArgs {
    lateinit var channel: Channel
}

@InvokeArg
class StageArgs {
    lateinit var uris: Array<String>
    lateinit var dest: String
}

@InvokeArg
class TextArgs {
    lateinit var text: String
    var title: String? = null
}

/**
 * Called from Ferry's Rust code only; see `plugins/ferry-mobile/src/lib.rs`. Things that happen
 * on Android's side (files shared to Ferry, the notification's Stop button, copy progress) go
 * back to Rust through the channel it passes to [watch].
 */
@TauriPlugin
class FerryMobilePlugin(private val activity: Activity) : Plugin(activity) {
    /** File copies run here, off the main thread, one at a time. */
    private val io = Executors.newSingleThreadExecutor()
    /** Copies of picked or shared files, kept apart so they don't wait behind saves. */
    private val staging = Executors.newSingleThreadExecutor()
    private var askedForNotifications = false
    private var events: Channel? = null
    /** Files shared to Ferry before Rust started listening. */
    private val pendingShared = mutableListOf<List<String>>()

    override fun load(webView: WebView) {
        super.load(webView)
        takeShared(activity.intent)
        TransferService.onAction = { action -> emit(JSObject().put("type", action)) }
    }

    override fun onNewIntent(intent: Intent) {
        takeShared(intent)
    }

    /** Files from another app's Share menu ("Share → Ferry"). */
    private fun takeShared(intent: Intent?) {
        if (intent == null) return
        val uris = when (intent.action) {
            Intent.ACTION_SEND ->
                listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE ->
                IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else -> return
        }
        // Handled once, even if the activity is recreated with the same intent.
        intent.action = Intent.ACTION_MAIN
        if (uris.isEmpty()) {
            emit(JSObject().put("type", "sharedNothing"))
            return
        }
        val strings = uris.map(Uri::toString)
        synchronized(pendingShared) {
            if (events == null) {
                pendingShared += strings
                return
            }
        }
        emitShared(strings)
    }

    private fun emitShared(uris: List<String>) {
        emit(JSObject().put("type", "shared").put("uris", JSArray(uris)))
    }

    private fun emit(event: JSObject) {
        events?.send(event)
    }

    /** Rust's event channel; files shared before it was set are delivered now. */
    @Command
    fun watch(invoke: Invoke) {
        val pending = synchronized(pendingShared) {
            events = invoke.parseArgs(WatchArgs::class.java).channel
            pendingShared.toList().also { pendingShared.clear() }
        }
        pending.forEach(::emitShared)
        invoke.resolve()
    }

    /** Let the user pick files to share. Resolves with their URIs; none if they cancel. */
    @Command
    fun pickFiles(invoke: Invoke) {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType("*/*")
            .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
        startActivityForResult(invoke, intent, "picked")
    }

    /** Let the user pick a folder to share. */
    @Command
    fun pickFolder(invoke: Invoke) {
        startActivityForResult(invoke, Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), "picked")
    }

    @ActivityCallback
    fun picked(invoke: Invoke, result: ActivityResult) {
        val data = result.data
        val uris = mutableListOf<String>()
        if (result.resultCode == Activity.RESULT_OK && data != null) {
            val clip = data.clipData
            if (clip != null) {
                for (i in 0 until clip.itemCount) clip.getItemAt(i).uri?.let { uris += it.toString() }
            } else {
                data.data?.let { uris += it.toString() }
            }
        }
        invoke.resolve(JSObject().put("uris", JSArray(uris)))
    }

    /** Copy picked or shared items into `dest`, reporting progress on the event channel. */
    @Command
    fun stage(invoke: Invoke) {
        val args = invoke.parseArgs(StageArgs::class.java)
        staging.execute {
            try {
                val result = Staging(activity.contentResolver).copy(args.uris.map(Uri::parse), File(args.dest)) { done, total ->
                    emit(JSObject().put("type", "stageProgress").put("doneBytes", done).put("totalBytes", total))
                }
                invoke.resolve(JSObject().put("paths", JSArray(result.paths)).put("skipped", result.skipped))
            } catch (e: Exception) {
                invoke.reject(e.message ?: "Couldn't copy the files to share")
            }
        }
    }

    /** Offer a text, such as a share link, to other apps through the share sheet. */
    @Command
    fun shareText(invoke: Invoke) {
        val args = invoke.parseArgs(TextArgs::class.java)
        val send = Intent(Intent.ACTION_SEND)
            .setType("text/plain")
            .putExtra(Intent.EXTRA_TEXT, args.text)
        args.title?.let { send.putExtra(Intent.EXTRA_SUBJECT, it) }
        start(invoke, Intent.createChooser(send, args.title), "No app on this phone can share the link")
    }

    /** Copy a received file into the shared Downloads collection. */
    @Command
    fun saveToDownloads(invoke: Invoke) {
        val args = invoke.parseArgs(SaveArgs::class.java)
        io.execute {
            try {
                invoke.resolve(save(File(args.path), args.relativeDir))
            } catch (e: Exception) {
                invoke.reject(e.message ?: "Couldn't save the file to Downloads")
            }
        }
    }

    private fun save(file: File, relativeDir: String): JSObject {
        val resolver = activity.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, file.name)
            put(MediaStore.Downloads.MIME_TYPE, mimeOf(file.name))
            put(MediaStore.Downloads.RELATIVE_PATH, relativeDir)
            put(MediaStore.Downloads.IS_PENDING, 1)
        }
        val collection = MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        val uri = resolver.insert(collection, values)
            ?: throw IOException("Android refused to create ${file.name} in Downloads")
        try {
            val out = resolver.openOutputStream(uri)
                ?: throw IOException("Couldn't write ${file.name} to Downloads")
            out.use { sink -> FileInputStream(file).use { it.copyTo(sink, 1 shl 16) } }
            val done = ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }
            resolver.update(uri, done, null, null)
        } catch (e: Exception) {
            resolver.delete(uri, null, null)
            throw e
        }
        // Android renames on clashes ("name (1).ext"); report the name it chose.
        val name = resolver.query(uri, arrayOf(MediaStore.Downloads.DISPLAY_NAME), null, null, null)
            ?.use { if (it.moveToFirst()) it.getString(0) else null } ?: file.name
        return JSObject().apply {
            put("uri", uri.toString())
            put("name", name)
        }
    }

    /** Open a saved file with the app the user picks for its type. */
    @Command
    fun open(invoke: Invoke) {
        val uri = Uri.parse(invoke.parseArgs(UriArgs::class.java).uri)
        val intent = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, activity.contentResolver.getType(uri) ?: "*/*")
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        start(invoke, intent, "No app on this phone can open this file")
    }

    /** Hand saved files to another app through the share sheet. */
    @Command
    fun share(invoke: Invoke) {
        val uris = invoke.parseArgs(UrisArgs::class.java).uris.map(Uri::parse)
        if (uris.isEmpty()) {
            invoke.reject("Nothing to share")
            return
        }
        val types = uris.map { activity.contentResolver.getType(it) ?: "*/*" }.distinct()
        val send = if (uris.size == 1) {
            Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, uris[0])
        } else {
            Intent(Intent.ACTION_SEND_MULTIPLE).putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(uris))
        }
        send.type = if (types.size == 1) types[0] else "*/*"
        // Read grants travel with the clip data, so the receiving app may read every file.
        send.clipData = ClipData.newRawUri(null, uris[0]).apply {
            uris.drop(1).forEach { addItem(ClipData.Item(it)) }
        }
        send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        start(invoke, Intent.createChooser(send, null), "No app on this phone can receive these files")
    }

    /** Open the system Downloads view, where saved shares live under Ferry. */
    @Command
    fun showDownloads(invoke: Invoke) {
        start(invoke, Intent(DownloadManager.ACTION_VIEW_DOWNLOADS), "This phone has no Downloads app")
    }

    /** Start, update or stop the transfer notification and the service that keeps Ferry running. */
    @Command
    fun setTransfer(invoke: Invoke) {
        val args = invoke.parseArgs(TransferArgs::class.java)
        try {
            if (args.active) {
                askForNotifications()
                TransferService.show(activity, args.title, args.text, args.progress, args.sharing)
            } else {
                TransferService.stop(activity)
            }
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject(e.message ?: "Couldn't update the transfer notification")
        }
    }

    /** Android 13+ hides notifications, the transfer one included, until the user allows them. */
    private fun askForNotifications() {
        if (Build.VERSION.SDK_INT < 33 || askedForNotifications) return
        askedForNotifications = true
        val permission = Manifest.permission.POST_NOTIFICATIONS
        if (ContextCompat.checkSelfPermission(activity, permission) != PackageManager.PERMISSION_GRANTED) {
            activity.runOnUiThread { ActivityCompat.requestPermissions(activity, arrayOf(permission), 7101) }
        }
    }

    private fun start(invoke: Invoke, intent: Intent, missing: String) {
        activity.runOnUiThread {
            try {
                activity.startActivity(intent)
                invoke.resolve()
            } catch (e: ActivityNotFoundException) {
                invoke.reject(missing)
            }
        }
    }

    private fun mimeOf(name: String): String {
        val ext = name.substringAfterLast('.', "").lowercase()
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "application/octet-stream"
    }
}
