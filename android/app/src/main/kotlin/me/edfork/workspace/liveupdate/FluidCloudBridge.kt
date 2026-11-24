package me.edfork.workspace.liveupdate

import android.content.Context
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicLong
import me.edfork.workspace.logging.WorkspaceLog

/**
 * Cross-process Fluid Cloud dispatcher usable identically from `me.edfork.workspace`,
 * hooked `com.android.systemui`, and hooked `com.coloros.oshare`.
 */
object FluidCloudBridge {
    private const val WORKSPACE_PACKAGE = "me.edfork.workspace"

    private val mainHandler = Handler(Looper.getMainLooper())
    private val tokenSeq = AtomicLong(0L)
    private val pendingIpcDismiss = ConcurrentHashMap<FluidSlot, Runnable>()

    private val ipcExecutor = Executors.newSingleThreadExecutor { r ->
        Thread(r, "workspace-fluid-bridge").apply { isDaemon = true }
    }

    fun showClipboardProgress(context: Context, action: String, peerName: String?) {
        val fluidAction = clipboardAction(action)
        val isPush = fluidAction == FluidAction.CLIP_PUSH
        val peerLabel = peerName?.takeIf { it.isNotBlank() } ?: "已配对电脑"
        val payload = FluidCloudPayload(
            slot = FluidSlot.CLIPBOARD,
            action = fluidAction,
            phase = FluidPhase.PROGRESS,
            capsuleText = FluidCloudFormatter.capsuleTextFor(fluidAction, FluidPhase.PROGRESS),
            title = if (isPush) "正在推送剪贴板" else "正在拉取剪贴板",
            detail = if (isPush) "发送至 $peerLabel..." else "从 $peerLabel 获取...",
            progress = -1,
            milestone = true,
            autoDismissMs = 0L,
        )
        dispatch(context, payload)
    }

    fun showClipboardDone(context: Context, action: String, peerName: String?, message: String) {
        val fluidAction = clipboardAction(action)
        val isPush = fluidAction == FluidAction.CLIP_PUSH
        val peerLabel = peerName?.takeIf { it.isNotBlank() }
        val detail = if (peerLabel != null) {
            "$peerLabel · $message"
        } else {
            message.ifBlank { if (isPush) "已同步到电脑" else "已写入手机剪贴板" }
        }
        val payload = FluidCloudPayload(
            slot = FluidSlot.CLIPBOARD,
            action = fluidAction,
            phase = FluidPhase.DONE,
            capsuleText = FluidCloudFormatter.capsuleTextFor(fluidAction, FluidPhase.DONE),
            title = if (isPush) "剪贴板推送完成" else "剪贴板拉取完成",
            detail = detail,
            progress = 100,
            milestone = true,
            autoDismissMs = FluidCloudController.CLIP_DONE_DISMISS_MS,
        )
        dispatch(context, payload)
    }

    fun showClipboardError(context: Context, action: String, message: String) {
        val fluidAction = clipboardAction(action)
        val isPush = fluidAction == FluidAction.CLIP_PUSH
        val payload = FluidCloudPayload(
            slot = FluidSlot.CLIPBOARD,
            action = fluidAction,
            phase = FluidPhase.ERROR,
            capsuleText = FluidCloudFormatter.capsuleTextFor(fluidAction, FluidPhase.ERROR),
            title = if (isPush) "剪贴板推送失败" else "剪贴板拉取失败",
            detail = message.ifBlank { "请检查局域网连接状态" },
            progress = 0,
            milestone = true,
            autoDismissMs = FluidCloudController.CLIP_ERROR_DISMISS_MS,
        )
        dispatch(context, payload)
    }

    fun showTransferProgress(
        context: Context,
        direction: String,
        peerName: String,
        summary: String,
        percent: Int,
        detail: String,
        isMilestone: Boolean,
    ) {
        val (slot, action) = transferSlotAndAction(direction)
        val isSend = action == FluidAction.FILE_SEND
        val peerLabel = peerName.ifBlank { "电脑" }
        val payload = FluidCloudPayload(
            slot = slot,
            action = action,
            phase = FluidPhase.PROGRESS,
            capsuleText = FluidCloudFormatter.capsuleTextFor(action, FluidPhase.PROGRESS, percent),
            title = if (isSend) "正在发送至 $peerLabel" else "正在接收来自 $peerLabel 的文件",
            detail = detail.ifBlank { summary },
            progress = percent,
            milestone = isMilestone,
            autoDismissMs = 0L,
        )
        dispatch(context, payload)
    }

    fun showTransferDone(
        context: Context,
        direction: String,
        peerName: String,
        summary: String,
        detail: String,
    ) {
        val (slot, action) = transferSlotAndAction(direction)
        val isSend = action == FluidAction.FILE_SEND
        val peerLabel = peerName.ifBlank { "电脑" }
        val payload = FluidCloudPayload(
            slot = slot,
            action = action,
            phase = FluidPhase.DONE,
            capsuleText = FluidCloudFormatter.capsuleTextFor(action, FluidPhase.DONE),
            title = if (isSend) "已发送到 $peerLabel" else "已接收来自 $peerLabel 的文件",
            detail = if (isSend) detail.ifBlank { summary } else "${detail.ifBlank { summary }} · 已存至 Download/寰宇工作台",
            progress = 100,
            milestone = true,
            autoDismissMs = FluidCloudController.TRANSFER_DONE_DISMISS_MS,
        )
        dispatch(context, payload)
    }

    fun showTransferCancelled(
        context: Context,
        direction: String,
        peerName: String,
        summary: String,
    ) {
        val (slot, action) = transferSlotAndAction(direction)
        val isSend = action == FluidAction.FILE_SEND
        val payload = FluidCloudPayload(
            slot = slot,
            action = action,
            phase = FluidPhase.CANCELLED,
            capsuleText = FluidCloudFormatter.capsuleTextFor(action, FluidPhase.CANCELLED),
            title = if (isSend) "已取消文件发送" else "已取消文件接收",
            detail = summary.ifBlank { peerName },
            progress = 0,
            milestone = true,
            autoDismissMs = FluidCloudController.TRANSFER_ERROR_DISMISS_MS,
        )
        dispatch(context, payload)
    }

    fun showTransferError(
        context: Context,
        direction: String,
        peerName: String,
        summary: String,
        errorMsg: String,
    ) {
        val (slot, action) = transferSlotAndAction(direction)
        val isSend = action == FluidAction.FILE_SEND
        val payload = FluidCloudPayload(
            slot = slot,
            action = action,
            phase = FluidPhase.ERROR,
            capsuleText = FluidCloudFormatter.capsuleTextFor(action, FluidPhase.ERROR),
            title = if (isSend) "文件发送失败" else "文件接收失败",
            detail = listOfNotNull(
                summary.takeIf { it.isNotBlank() },
                errorMsg.takeIf { it.isNotBlank() },
                peerName.takeIf { it.isNotBlank() && summary.isBlank() && errorMsg.isBlank() },
            ).joinToString(" · ").ifBlank { "传输异常，请检查局域网连接" },
            progress = 0,
            milestone = true,
            autoDismissMs = FluidCloudController.TRANSFER_ERROR_DISMISS_MS,
        )
        dispatch(context, payload)
    }

    private fun clipboardAction(action: String): FluidAction =
        if (action.equals("pull", ignoreCase = true)) FluidAction.CLIP_PULL else FluidAction.CLIP_PUSH

    private fun transferSlotAndAction(direction: String): Pair<FluidSlot, FluidAction> =
        if (direction.equals("receive", ignoreCase = true)) {
            FluidSlot.FILE_RECEIVE to FluidAction.FILE_RECEIVE
        } else {
            FluidSlot.FILE_SEND to FluidAction.FILE_SEND
        }

    private fun nextToken(): Long {
        val t = (System.nanoTime() shl 12) xor tokenSeq.incrementAndGet()
        return if (t == 0L) 1L else t
    }

    private fun dispatch(context: Context, payload: FluidCloudPayload) {
        val appCtx = context.applicationContext ?: context
        val token = nextToken()
        val taggedPayload = payload.copy(token = token)
        if (appCtx.packageName == WORKSPACE_PACKAGE) {
            runCatching {
                FluidCloudController.postOrUpdate(appCtx, taggedPayload)
            }.onFailure { err ->
                WorkspaceLog.warn("fluid_bridge", "Local FluidCloud dispatch failed", err)
            }
            return
        }

        pendingIpcDismiss.remove(taggedPayload.slot)?.let { mainHandler.removeCallbacks(it) }
        val bundle = taggedPayload.toBundle()
        ipcExecutor.execute {
            runCatching {
                appCtx.contentResolver.call(
                    FluidCloudProvider.CONTENT_URI,
                    FluidCloudProvider.METHOD_POST,
                    null,
                    bundle,
                )
            }.onFailure { err ->
                WorkspaceLog.warn("fluid_bridge", "IPC FluidCloud dispatch failed from ${appCtx.packageName}", err)
            }
        }
        if (taggedPayload.autoDismissMs > 0L) {
            val slot = taggedPayload.slot
            val task = Runnable {
                pendingIpcDismiss.remove(slot)
                ipcExecutor.execute {
                    runCatching {
                        val dismissBundle = Bundle().apply {
                            putString(FluidCloudPayload.KEY_SLOT, slot.name)
                            putLong(FluidCloudPayload.KEY_TOKEN, token)
                        }
                        appCtx.contentResolver.call(
                            FluidCloudProvider.CONTENT_URI,
                            FluidCloudProvider.METHOD_DISMISS,
                            slot.name,
                            dismissBundle,
                        )
                    }.onFailure { err ->
                        WorkspaceLog.warn("fluid_bridge", "IPC FluidCloud dismiss failed from ${appCtx.packageName}", err)
                    }
                }
            }
            pendingIpcDismiss[slot] = task
            mainHandler.postDelayed(task, taggedPayload.autoDismissMs + 150L)
        }
    }
}
