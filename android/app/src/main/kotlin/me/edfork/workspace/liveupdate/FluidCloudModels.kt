package me.edfork.workspace.liveupdate

import android.os.Bundle
import java.util.Locale

enum class FluidSlot(val tag: String, val notificationId: Int) {
    CLIPBOARD("ws_clip", 45871),
    FILE_SEND("ws_send", 45872),
    FILE_RECEIVE("ws_recv", 45873);

    companion object {
        fun fromName(raw: String?): FluidSlot =
            entries.firstOrNull { it.name == raw } ?: CLIPBOARD
    }
}

enum class FluidPhase {
    PROGRESS,
    DONE,
    ERROR,
    CANCELLED;

    companion object {
        fun fromName(raw: String?): FluidPhase =
            entries.firstOrNull { it.name == raw } ?: PROGRESS
    }
}

enum class FluidAction {
    CLIP_PUSH,
    CLIP_PULL,
    FILE_SEND,
    FILE_RECEIVE;

    companion object {
        fun fromName(raw: String?): FluidAction =
            entries.firstOrNull { it.name == raw } ?: CLIP_PUSH
    }
}

data class FluidCloudPayload(
    val slot: FluidSlot,
    val action: FluidAction,
    val phase: FluidPhase,
    val capsuleText: String,
    val title: String,
    val detail: String,
    val progress: Int = -1,
    val milestone: Boolean = true,
    val autoDismissMs: Long = 0L,
    val token: Long = 0L,
) {
    fun toBundle(): Bundle = Bundle().apply {
        putString(KEY_SLOT, slot.name)
        putString(KEY_ACTION, action.name)
        putString(KEY_PHASE, phase.name)
        putString(KEY_CAPSULE_TEXT, capsuleText)
        putString(KEY_TITLE, title)
        putString(KEY_DETAIL, detail)
        putInt(KEY_PROGRESS, progress)
        putBoolean(KEY_MILESTONE, milestone)
        putLong(KEY_AUTO_DISMISS_MS, autoDismissMs)
        putLong(KEY_TOKEN, token)
    }

    companion object {
        const val KEY_SLOT = "slot"
        const val KEY_ACTION = "action"
        const val KEY_PHASE = "phase"
        const val KEY_CAPSULE_TEXT = "capsule_text"
        const val KEY_TITLE = "title"
        const val KEY_DETAIL = "detail"
        const val KEY_PROGRESS = "progress"
        const val KEY_MILESTONE = "milestone"
        const val KEY_AUTO_DISMISS_MS = "auto_dismiss_ms"
        const val KEY_TOKEN = "token"

        fun fromBundle(bundle: Bundle?): FluidCloudPayload? {
            if (bundle == null) return null
            val title = bundle.getString(KEY_TITLE)?.takeIf { it.isNotBlank() } ?: return null
            return FluidCloudPayload(
                slot = FluidSlot.fromName(bundle.getString(KEY_SLOT)),
                action = FluidAction.fromName(bundle.getString(KEY_ACTION)),
                phase = FluidPhase.fromName(bundle.getString(KEY_PHASE)),
                capsuleText = bundle.getString(KEY_CAPSULE_TEXT).orEmpty().ifBlank { title },
                title = title,
                detail = bundle.getString(KEY_DETAIL).orEmpty(),
                progress = bundle.getInt(KEY_PROGRESS, -1),
                milestone = bundle.getBoolean(KEY_MILESTONE, true),
                autoDismissMs = bundle.getLong(KEY_AUTO_DISMISS_MS, 0L),
                token = bundle.getLong(KEY_TOKEN, 0L),
            )
        }
    }
}

object FluidCloudFormatter {
    fun computePercent(bytesTransferred: Long, totalBytes: Long): Int {
        if (totalBytes <= 0L) return -1
        val clamped = bytesTransferred.coerceIn(0L, totalBytes)
        return ((clamped * 100L) / totalBytes).toInt().coerceIn(0, 99)
    }

    fun capsuleTextFor(action: FluidAction, phase: FluidPhase, percent: Int = -1): String =
        when (action) {
            FluidAction.CLIP_PUSH -> when (phase) {
                FluidPhase.PROGRESS -> "推送中"
                FluidPhase.DONE -> "已推送"
                FluidPhase.ERROR -> "推送失败"
                FluidPhase.CANCELLED -> "已取消"
            }
            FluidAction.CLIP_PULL -> when (phase) {
                FluidPhase.PROGRESS -> "拉取中"
                FluidPhase.DONE -> "已拉取"
                FluidPhase.ERROR -> "拉取失败"
                FluidPhase.CANCELLED -> "已取消"
            }
            FluidAction.FILE_SEND -> when (phase) {
                FluidPhase.PROGRESS -> if (percent in 1..99) "发送 $percent%" else "发送中"
                FluidPhase.DONE -> "已发送"
                FluidPhase.CANCELLED -> "已取消"
                FluidPhase.ERROR -> "发送失败"
            }
            FluidAction.FILE_RECEIVE -> when (phase) {
                FluidPhase.PROGRESS -> if (percent in 1..99) "接收 $percent%" else "接收中"
                FluidPhase.DONE -> "已接收"
                FluidPhase.CANCELLED -> "已取消"
                FluidPhase.ERROR -> "接收失败"
            }
        }

    fun formatHumanSize(bytes: Long): String {
        if (bytes < 0L) return "未知大小"
        if (bytes < 1024L) return "$bytes B"
        val kb = bytes / 1024.0
        if (kb < 1024.0) return String.format(Locale.US, "%.1f KB", kb)
        val mb = kb / 1024.0
        if (mb < 1024.0) return String.format(Locale.US, "%.1f MB", mb)
        val gb = mb / 1024.0
        return String.format(Locale.US, "%.2f GB", gb)
    }

    fun formatTransferDetail(
        summary: String,
        bytesTransferred: Long,
        totalBytes: Long,
        percent: Int,
        fallbackDetail: String? = null,
    ): String {
        val base = summary.ifBlank { "文件传输" }
        return when {
            totalBytes > 0L && percent in 0..99 ->
                "$base · ${formatHumanSize(bytesTransferred)} / ${formatHumanSize(totalBytes)} ($percent%)"
            totalBytes > 0L ->
                "$base · ${formatHumanSize(totalBytes)}"
            !fallbackDetail.isNullOrBlank() ->
                "$base · $fallbackDetail"
            else -> base
        }
    }
}
