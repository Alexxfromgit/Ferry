package com.xenvaultinteractive.ferry

import android.app.Activity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.google.android.play.core.appupdate.AppUpdateInfo
import com.google.android.play.core.appupdate.AppUpdateManagerFactory
import com.google.android.play.core.appupdate.AppUpdateOptions
import com.google.android.play.core.install.model.AppUpdateType
import com.google.android.play.core.install.model.InstallStatus
import com.google.android.play.core.install.model.UpdateAvailability

/**
 * Updates for the Google Play build, through Play's in-app updates. Play downloads a flexible
 * update in the background while Ferry keeps running; installing it restarts the app. An update
 * marked high priority (4 or 5) or one that has been waiting 30 days is installed right away
 * instead, Play showing its own full-screen flow.
 */
@TauriPlugin
class UpdatePlugin(private val activity: Activity) : Plugin(activity) {
    private val manager = AppUpdateManagerFactory.create(activity)

    @InvokeArg
    class CheckArgs {
        var automatic: Boolean = false
    }

    /** The version Play has finished downloading, if any. */
    @Command
    fun ready(invoke: Invoke) {
        manager.appUpdateInfo
            .addOnSuccessListener { invoke.resolve(readyVersion(it)) }
            .addOnFailureListener { invoke.resolve(readyVersion(null)) }
    }

    /** Ask Play for an update and start downloading it. Resolves with a version once downloaded. */
    @Command
    fun check(invoke: Invoke) {
        manager.appUpdateInfo
            .addOnSuccessListener { info ->
                if (info.installStatus() == InstallStatus.DOWNLOADED) {
                    invoke.resolve(readyVersion(info))
                    return@addOnSuccessListener
                }
                val available = info.updateAvailability() == UpdateAvailability.UPDATE_AVAILABLE ||
                    info.updateAvailability() == UpdateAvailability.DEVELOPER_TRIGGERED_UPDATE_IN_PROGRESS
                if (!available) {
                    invoke.resolve(readyVersion(null))
                    return@addOnSuccessListener
                }
                val urgent = info.updatePriority() >= 4 || (info.clientVersionStalenessDays() ?: 0) >= 30
                val type = if (urgent && info.isUpdateTypeAllowed(AppUpdateType.IMMEDIATE)) {
                    AppUpdateType.IMMEDIATE
                } else {
                    AppUpdateType.FLEXIBLE
                }
                if (!info.isUpdateTypeAllowed(type)) {
                    invoke.resolve(readyVersion(null))
                    return@addOnSuccessListener
                }
                activity.runOnUiThread {
                    manager.startUpdateFlow(info, activity, AppUpdateOptions.defaultOptions(type))
                    // Play downloads in the background; `ready` reports it once finished.
                    invoke.resolve(readyVersion(null))
                }
            }
            .addOnFailureListener { invoke.reject(it.message ?: "Google Play couldn't check for updates") }
    }

    /** Restart into the update Play downloaded. */
    @Command
    fun install(invoke: Invoke) {
        manager.completeUpdate()
            .addOnSuccessListener { invoke.resolve() }
            .addOnFailureListener { invoke.reject(it.message ?: "Google Play couldn't install the update") }
    }

    private fun readyVersion(info: AppUpdateInfo?): JSObject = JSObject().apply {
        val downloaded = info?.installStatus() == InstallStatus.DOWNLOADED
        put("readyVersion", if (downloaded) info?.availableVersionCode()?.let(::versionName) else null)
    }

    /** Tauri's version codes are major * 1,000,000 + minor * 1,000 + patch. */
    private fun versionName(code: Int) = "${code / 1_000_000}.${code / 1_000 % 1_000}.${code % 1_000}"
}
