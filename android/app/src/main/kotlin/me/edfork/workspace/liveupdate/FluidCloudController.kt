package me.edfork.workspace.liveupdate

import android.Manifest
import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong
import me.edfork.workspace.R
import me.edfork.workspace.logging.WorkspaceLog
import me.edfork.workspace.ui.MainActivity

/**
 * Posts Android 16/17 Promoted Live Updates notifications inside `me.edfork.workspace`
 * so ColorOS 17 renders clipboard & file transfer states natively on Fluid Cloud (流体云).
 */
object FluidCloudController {
    const val CHANNEL_ID = "workspace_fluid_cloud"
    private const val CHANNEL_NAME = "寰宇工作台流体云"

    private const val COLOR_PROGRESS = 0xFF006A60.toInt()
    private const val COLOR_DONE = 0xFF1B8A5A.toInt()
    private const val COLOR_ERROR = 0xFFBA1A1A.toInt()
    private const val COLOR_CANCELLED = 0xFF795548.toInt()

    const val CLIP_DONE_DISMISS_MS = 5000L
    const val CLIP_ERROR_DISMISS_MS = 5000L
    const val TRANSFER_DONE_DISMISS_MS = 5000L
    const val TRANSFER_ERROR_DISMISS_MS = 5000L
    private const val STALE_PROGRESS_CLIP_MS = 20_000L
    private const val STALE_PROGRESS_TRANSFER_MS = 60_000L

    private val mainHandler = Handler(Looper.getMainLooper())
    private val generations = ConcurrentHashMap<FluidSlot, AtomicLong>()
    private val lastTokens = ConcurrentHashMap<FluidSlot, Long>()
    private val pendingDismiss = ConcurrentHashMap<FluidSlot, Runnable>()

    fun ensureChannel(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        val existing = nm.getNotificationChannel(CHANNEL_ID)
        if (existing != null && existing.importance >= NotificationManager.IMPORTANCE_DEFAULT) {
            return
        }
        val channel = NotificationChannel(
            CHANNEL_ID,
            CHANNEL_NAME,
            NotificationManager.IMPORTANCE_DEFAULT,
        ).apply {
            description = "显示剪贴板推拉及局域网文件收发的实时流体云进度与结果"
            setSound(null, null)
            enableVibration(false)
            enableLights(false)
            setShowBadge(false)
            setBypassDnd(false)
        }
        nm.createNotificationChannel(channel)
    }

    fun areNotificationsEnabled(context: Context): Boolean {
        val nm = context.getSystemService(NotificationManager::class.java) ?: return false
        if (!nm.areNotificationsEnabled()) return false
        if (Build.VERSION.SDK_INT >= 33) {
            val granted = context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
                PackageManager.PERMISSION_GRANTED
            if (!granted) return false
        }
        return true
    }

    fun canPostPromoted(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < 36) return false
        val nm = context.getSystemService(NotificationManager::class.java) ?: return false
        return runCatching { nm.canPostPromotedNotifications() }.getOrDefault(false)
    }

    @SuppressLint("NewApi")
    fun postOrUpdate(context: Context, payload: FluidCloudPayload) {
        val appCtx = context.applicationContext ?: context
        if (!areNotificationsEnabled(appCtx)) {
            WorkspaceLog.warn(
                "fluid_cloud",
                "Notifications disabled or missing POST_NOTIFICATIONS; skipping ${payload.slot}",
            )
            return
        }

        ensureChannel(appCtx)
        val nm = appCtx.getSystemService(NotificationManager::class.java) ?: return

        val gen = nextGeneration(payload.slot)
        if (payload.token != 0L) {
            lastTokens[payload.slot] = payload.token
        } else {
            lastTokens.remove(payload.slot)
        }
        cancelPendingDismiss(payload.slot)

        val dismissDelayMs = when {
            payload.autoDismissMs > 0L -> payload.autoDismissMs
            payload.phase == FluidPhase.PROGRESS -> when (payload.slot) {
                FluidSlot.CLIPBOARD -> STALE_PROGRESS_CLIP_MS
                FluidSlot.FILE_SEND, FluidSlot.FILE_RECEIVE -> STALE_PROGRESS_TRANSFER_MS
            }
            else -> 0L
        }

        val launchIntent = Intent(appCtx, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
        }
        val contentPendingIntent = PendingIntent.getActivity(
            appCtx,
            payload.slot.notificationId,
            launchIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val accentColor = when (payload.phase) {
            FluidPhase.PROGRESS -> COLOR_PROGRESS
            FluidPhase.DONE -> COLOR_DONE
            FluidPhase.ERROR -> COLOR_ERROR
            FluidPhase.CANCELLED -> COLOR_CANCELLED
        }

        val builder = Notification.Builder(appCtx, CHANNEL_ID)
            .setSmallIcon(iconResFor(payload.action, payload.phase))
            .setContentTitle(payload.title.ifBlank { "寰宇工作台" })
            .setContentText(payload.detail.ifBlank { payload.capsuleText })
            .setSubText("寰宇工作台")
            // Keep ongoing=true even in DONE/ERROR states so Android 16/17 retains FLAG_PROMOTED_ONGOING
            // during the brief completion hold window before auto-cancelling.
            .setOngoing(true)
            .setOnlyAlertOnce(!payload.milestone)
            .setColor(accentColor)
            .setCategory(Notification.CATEGORY_PROGRESS)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setContentIntent(contentPendingIntent)

        if (dismissDelayMs > 0L) {
            builder.setTimeoutAfter(dismissDelayMs)
        }

        if (Build.VERSION.SDK_INT >= 36) {
            builder.setRequestPromotedOngoing(true)
            builder.setShortCriticalText(payload.capsuleText)
            builder.addExtras(
                Bundle().apply {
                    putString("oplusLiveAlertOptions", "{\"isMilestone\":${payload.milestone}}")
                },
            )
            when (payload.phase) {
                FluidPhase.PROGRESS -> {
                    val style = if (payload.progress in 0..100) {
                        Notification.ProgressStyle()
                            .setProgress(payload.progress)
                            .setStyledByProgress(true)
                            .addProgressSegment(
                                Notification.ProgressStyle.Segment(100).setColor(COLOR_PROGRESS),
                            )
                    } else {
                        Notification.ProgressStyle().setProgressIndeterminate(true)
                    }
                    builder.setStyle(style)
                }
                FluidPhase.DONE -> {
                    val style = Notification.ProgressStyle()
                        .setProgress(100)
                        .setStyledByProgress(true)
                        .addProgressSegment(
                            Notification.ProgressStyle.Segment(100).setColor(COLOR_DONE),
                        )
                    builder.setStyle(style)
                }
                FluidPhase.ERROR, FluidPhase.CANCELLED -> {
                    // Use StandardStyle (promotable in ColorOS 17 _stylesForPromoteRule) without ProgressStyle
                    // so failures/cancellations display clearly with warning icon and error description.
                }
            }
        } else {
            when (payload.phase) {
                FluidPhase.PROGRESS -> builder.setProgress(100, payload.progress.coerceIn(0, 100), payload.progress < 0)
                FluidPhase.DONE -> builder.setProgress(100, 100, false)
                FluidPhase.ERROR, FluidPhase.CANCELLED -> builder.setProgress(0, 0, false)
            }
        }

        nm.notify(payload.slot.tag, payload.slot.notificationId, builder.build())

        if (dismissDelayMs > 0L) {
            scheduleDismiss(appCtx, payload.slot, gen, dismissDelayMs)
        }
    }

    fun dismiss(context: Context, slot: FluidSlot, expectedToken: Long = 0L) {
        val appCtx = context.applicationContext ?: context
        if (expectedToken != 0L) {
            val activeToken = lastTokens[slot]
            if (activeToken != expectedToken) {
                return
            }
        }
        nextGeneration(slot)
        lastTokens.remove(slot)
        cancelPendingDismiss(slot)
        appCtx.getSystemService(NotificationManager::class.java)?.cancel(slot.tag, slot.notificationId)
    }

    private fun nextGeneration(slot: FluidSlot): Long =
        generations.computeIfAbsent(slot) { AtomicLong(0L) }.incrementAndGet()

    private fun currentGeneration(slot: FluidSlot): Long =
        generations[slot]?.get() ?: 0L

    private fun cancelPendingDismiss(slot: FluidSlot) {
        pendingDismiss.remove(slot)?.let { mainHandler.removeCallbacks(it) }
    }

    private fun scheduleDismiss(context: Context, slot: FluidSlot, expectedGen: Long, delayMs: Long) {
        val task = Runnable {
            if (currentGeneration(slot) == expectedGen) {
                pendingDismiss.remove(slot)
                context.getSystemService(NotificationManager::class.java)?.cancel(slot.tag, slot.notificationId)
            }
        }
        pendingDismiss[slot] = task
        mainHandler.postDelayed(task, delayMs)
    }

    private fun iconResFor(action: FluidAction, phase: FluidPhase): Int =
        when (phase) {
            FluidPhase.DONE -> R.drawable.ic_fluid_done
            FluidPhase.ERROR, FluidPhase.CANCELLED -> R.drawable.ic_fluid_error
            FluidPhase.PROGRESS -> when (action) {
                FluidAction.CLIP_PUSH -> R.drawable.ic_fluid_clip_push
                FluidAction.CLIP_PULL -> R.drawable.ic_fluid_clip_pull
                FluidAction.FILE_SEND -> R.drawable.ic_fluid_file_send
                FluidAction.FILE_RECEIVE -> R.drawable.ic_fluid_file_receive
            }
        }
}
