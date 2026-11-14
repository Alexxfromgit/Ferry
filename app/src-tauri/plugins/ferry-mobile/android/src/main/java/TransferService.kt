package com.xenvaultinteractive.ferry.mobile

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat

/**
 * A foreground service that does no work itself: while it runs, Android keeps Ferry's process
 * (and so its network connections) alive with the screen off, and shows what Ferry is
 * transferring. It runs while files are being received, and while shares are on, so the phone
 * stays reachable for whoever opens the link. It also holds the CPU awake and keeps Wi-Fi out
 * of power saving, which otherwise slows a transfer to a crawl once the screen turns off.
 */
class TransferService : Service() {
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        wakeLock = getSystemService(PowerManager::class.java)
            .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Ferry:transfer")
            .apply {
                setReferenceCounted(false)
                acquire(MAX_HOLD_MS)
            }
        @Suppress("DEPRECATION") // The replacement (low latency) only works with the screen on.
        wifiLock = applicationContext.getSystemService(WifiManager::class.java)
            ?.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "Ferry:transfer")
            ?.apply {
                setReferenceCounted(false)
                acquire()
            }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP_SHARING) {
            // The notification's button. Ferry turns its shares off, then updates or stops this.
            onAction?.invoke("stopSharing")
            return START_NOT_STICKY
        }
        val notification = build(
            this,
            intent?.getStringExtra(EXTRA_TITLE) ?: "",
            intent?.getStringExtra(EXTRA_TEXT) ?: "",
            intent?.takeIf { it.hasExtra(EXTRA_PROGRESS) }?.getIntExtra(EXTRA_PROGRESS, 0),
            intent?.getBooleanExtra(EXTRA_SHARING, false) ?: false,
        )
        startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        running = true
        return START_NOT_STICKY
    }

    // Android 15 limits data-sync services to six hours a day. Stop, tell the user, and let
    // Ferry turn its shares off, since the phone can't be reached in the background any more.
    override fun onTimeout(startId: Int, fgsType: Int) {
        val manager = getSystemService(NotificationManager::class.java)
        manager.notify(
            TIMEOUT_NOTIFICATION_ID,
            NotificationCompat.Builder(this, CHANNEL_ID)
                .setSmallIcon(R.drawable.ferry_ic_transfer)
                .setContentTitle(getString(R.string.ferry_timeout_title))
                .setContentText(getString(R.string.ferry_timeout_text))
                .setStyle(NotificationCompat.BigTextStyle().bigText(getString(R.string.ferry_timeout_text)))
                .setContentIntent(openApp(this))
                .setAutoCancel(true)
                .build(),
        )
        onAction?.invoke("timeout")
        stopSelf()
    }

    override fun onDestroy() {
        running = false
        wifiLock?.takeIf { it.isHeld }?.release()
        wakeLock?.takeIf { it.isHeld }?.release()
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL_ID = "ferry-transfers"
        private const val NOTIFICATION_ID = 7100
        private const val TIMEOUT_NOTIFICATION_ID = 7102
        private const val EXTRA_TITLE = "title"
        private const val EXTRA_TEXT = "text"
        private const val EXTRA_PROGRESS = "progress"
        private const val EXTRA_SHARING = "sharing"
        private const val ACTION_STOP_SHARING = "com.xenvaultinteractive.ferry.mobile.STOP_SHARING"
        /** Matches Android 15's daily limit for data-sync services. */
        private const val MAX_HOLD_MS = 6L * 60 * 60 * 1000

        @Volatile
        private var running = false

        /** Set by the plugin: receives "stopSharing" and "timeout". */
        @Volatile
        var onAction: ((String) -> Unit)? = null

        fun show(context: Context, title: String, text: String, progress: Int?, sharing: Boolean) {
            if (running) {
                // Already in the foreground: just update the notification.
                context.getSystemService(NotificationManager::class.java)
                    .notify(NOTIFICATION_ID, build(context, title, text, progress, sharing))
                return
            }
            val intent = Intent(context, TransferService::class.java)
                .putExtra(EXTRA_TITLE, title)
                .putExtra(EXTRA_TEXT, text)
                .putExtra(EXTRA_SHARING, sharing)
            if (progress != null) intent.putExtra(EXTRA_PROGRESS, progress)
            ContextCompat.startForegroundService(context, intent)
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, TransferService::class.java))
        }

        private fun openApp(context: Context): PendingIntent? =
            context.packageManager.getLaunchIntentForPackage(context.packageName)?.let {
                PendingIntent.getActivity(context, 0, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            }

        private fun build(context: Context, title: String, text: String, progress: Int?, sharing: Boolean): Notification {
            val manager = context.getSystemService(NotificationManager::class.java)
            if (manager.getNotificationChannel(CHANNEL_ID) == null) {
                manager.createNotificationChannel(
                    NotificationChannel(
                        CHANNEL_ID,
                        context.getString(R.string.ferry_transfers_channel),
                        NotificationManager.IMPORTANCE_LOW,
                    ).apply { description = context.getString(R.string.ferry_transfers_channel_description) }
                )
            }
            val builder = NotificationCompat.Builder(context, CHANNEL_ID)
                .setSmallIcon(R.drawable.ferry_ic_transfer)
                .setContentTitle(title)
                .setContentText(text)
                .setContentIntent(openApp(context))
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setSilent(true)
                .setCategory(NotificationCompat.CATEGORY_PROGRESS)
                .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            // Receiving shows its progress (or an indeterminate bar while connecting); sharing
            // with nobody downloading shows no bar at all.
            if (progress != null || !sharing) builder.setProgress(100, progress ?: 0, progress == null)
            if (sharing) {
                val stop = PendingIntent.getService(
                    context,
                    1,
                    Intent(context, TransferService::class.java).setAction(ACTION_STOP_SHARING),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                )
                builder.addAction(0, context.getString(R.string.ferry_stop_sharing), stop)
            }
            return builder.build()
        }
    }
}
