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
 * (and so its network connections) alive with the screen off, and shows transfer progress.
 * It also holds the CPU awake and keeps Wi-Fi out of power saving, which otherwise slows a
 * transfer to a crawl once the screen turns off.
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
        val notification = build(
            this,
            intent?.getStringExtra(EXTRA_TITLE) ?: "",
            intent?.getStringExtra(EXTRA_TEXT) ?: "",
            intent?.takeIf { it.hasExtra(EXTRA_PROGRESS) }?.getIntExtra(EXTRA_PROGRESS, 0),
        )
        startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        running = true
        return START_NOT_STICKY
    }

    // Android 15 limits data-sync services to six hours a day; stop cleanly when told to.
    override fun onTimeout(startId: Int, fgsType: Int) {
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
        private const val EXTRA_TITLE = "title"
        private const val EXTRA_TEXT = "text"
        private const val EXTRA_PROGRESS = "progress"
        /** Matches Android 15's daily limit for data-sync services. */
        private const val MAX_HOLD_MS = 6L * 60 * 60 * 1000

        @Volatile
        private var running = false

        fun show(context: Context, title: String, text: String, progress: Int?) {
            if (running) {
                // Already in the foreground: just update the notification.
                context.getSystemService(NotificationManager::class.java)
                    .notify(NOTIFICATION_ID, build(context, title, text, progress))
                return
            }
            val intent = Intent(context, TransferService::class.java)
                .putExtra(EXTRA_TITLE, title)
                .putExtra(EXTRA_TEXT, text)
            if (progress != null) intent.putExtra(EXTRA_PROGRESS, progress)
            ContextCompat.startForegroundService(context, intent)
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, TransferService::class.java))
        }

        private fun build(context: Context, title: String, text: String, progress: Int?): Notification {
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
            val open = context.packageManager.getLaunchIntentForPackage(context.packageName)?.let {
                PendingIntent.getActivity(context, 0, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            }
            return NotificationCompat.Builder(context, CHANNEL_ID)
                .setSmallIcon(R.drawable.ferry_ic_transfer)
                .setContentTitle(title)
                .setContentText(text)
                .setContentIntent(open)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setSilent(true)
                .setCategory(NotificationCompat.CATEGORY_PROGRESS)
                .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
                .setProgress(100, progress ?: 0, progress == null)
                .build()
        }
    }
}
