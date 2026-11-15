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
import com.google.android.play.core.install.InstallStateUpdatedListener
import com.google.android.play.core.install.model.AppUpdateType
import com.google.android.play.core.install.model.InstallStatus
import com.google.android.play.core.install.model.UpdateAvailability

/**
 * Updates for the Google Play build, through Play's in-app updates. Play downloads a flexible
 * update in the background while Ferry keeps running; installing it restarts the app. An update
 * marked high priority (4 or 5) or one that has been waiting 30 days is installed right away
 * instead, Play showing its own full-screen flow, but only when Ferry says nothing would be cut
 * off (`allowImmediate`).
 *
 * A check that starts a flexible download resolves once Play reports how it ended: with the
 * version when it finished, or with none when the user declined or cancelled it.
 */
@TauriPlugin
class UpdatePlugin(private val activity: Activity) : Plugin(activity) {
    private val manager = AppUpdateManagerFactory.create(activity)

    /** Checks waiting for Play's flexible download to end. Touched on the main thread only. */
    private val waiting = mutableListOf<Invoke>()

    /** The version code of the update being downloaded. */
    private var downloading: Int? = null

    private val listener = InstallStateUpdatedListener { state ->
        when (state.installStatus()) {
            InstallStatus.DOWNLOADED -> settle { it.resolve(readyVersion(downloading)) }
            InstallStatus.CANCELED -> settle { it.resolve(readyVersion(null)) }
            InstallStatus.FAILED -> settle { it.reject("Google Play couldn't download the update") }
            else -> {}
        }
    }

    init {
        manager.registerListener(listener)
    }

    @InvokeArg
    class CheckArgs {
        var automatic: Boolean = false
        var allowImmediate: Boolean = false
    }

    /** The version Play has finished downloading, if any. */
    @Command
    fun ready(invoke: Invoke) {
        manager.appUpdateInfo
            .addOnSuccessListener { invoke.resolve(readyVersion(downloadedCode(it))) }
            .addOnFailureListener { invoke.resolve(readyVersion(null)) }
    }

    /** Ask Play for an update and download it. Resolves with a version once downloaded. */
    @Command
    fun check(invoke: Invoke) {
        val args = invoke.parseArgs(CheckArgs::class.java)
        manager.appUpdateInfo
            .addOnSuccessListener { info ->
                when (info.installStatus()) {
                    InstallStatus.DOWNLOADED -> {
                        invoke.resolve(readyVersion(info.availableVersionCode()))
                        return@addOnSuccessListener
                    }
                    // Already downloading (an earlier check, or before Ferry restarted).
                    InstallStatus.PENDING, InstallStatus.DOWNLOADING -> {
                        downloading = info.availableVersionCode()
                        waiting.add(invoke)
                        return@addOnSuccessListener
                    }
                    else -> {}
                }
                val available = info.updateAvailability() == UpdateAvailability.UPDATE_AVAILABLE ||
                    info.updateAvailability() == UpdateAvailability.DEVELOPER_TRIGGERED_UPDATE_IN_PROGRESS
                if (!available) {
                    invoke.resolve(readyVersion(null))
                    return@addOnSuccessListener
                }
                val urgent = info.updatePriority() >= 4 || (info.clientVersionStalenessDays() ?: 0) >= 30
                val type = if (urgent && args.allowImmediate && info.isUpdateTypeAllowed(AppUpdateType.IMMEDIATE)) {
                    AppUpdateType.IMMEDIATE
                } else {
                    AppUpdateType.FLEXIBLE
                }
                if (!info.isUpdateTypeAllowed(type)) {
                    invoke.resolve(readyVersion(null))
                    return@addOnSuccessListener
                }
                activity.runOnUiThread { startFlow(invoke, info, type) }
            }
            .addOnFailureListener { invoke.reject(it.message ?: "Google Play couldn't check for updates") }
    }

    private fun startFlow(invoke: Invoke, info: AppUpdateInfo, type: Int) {
        if (type == AppUpdateType.FLEXIBLE) {
            downloading = info.availableVersionCode()
            waiting.add(invoke)
        }
        manager.startUpdateFlow(info, activity, AppUpdateOptions.defaultOptions(type))
            .addOnSuccessListener { result ->
                // RESULT_OK: the user accepted and Play is downloading; the listener settles.
                if (result != Activity.RESULT_OK) {
                    settle { it.resolve(readyVersion(null)) }
                    if (type == AppUpdateType.IMMEDIATE) invoke.resolve(readyVersion(null))
                }
            }
            .addOnFailureListener { error ->
                val message = error.message ?: "Google Play couldn't start the update"
                settle { it.reject(message) }
                if (type == AppUpdateType.IMMEDIATE) invoke.reject(message)
            }
    }

    /** Answer every check waiting for the flexible download. */
    private fun settle(answer: (Invoke) -> Unit) {
        val invokes = waiting.toList()
        waiting.clear()
        invokes.forEach(answer)
    }

    /** Restart into the update Play downloaded. */
    @Command
    fun install(invoke: Invoke) {
        manager.completeUpdate()
            .addOnSuccessListener { invoke.resolve() }
            .addOnFailureListener { invoke.reject(it.message ?: "Google Play couldn't install the update") }
    }

    private fun downloadedCode(info: AppUpdateInfo): Int? =
        info.availableVersionCode().takeIf { info.installStatus() == InstallStatus.DOWNLOADED }

    private fun readyVersion(code: Int?): JSObject = JSObject().apply {
        put("readyVersion", code?.let(::versionName))
    }

    /** Tauri's version codes are major * 1,000,000 + minor * 1,000 + patch. */
    private fun versionName(code: Int) = "${code / 1_000_000}.${code / 1_000 % 1_000}.${code % 1_000}"
}
