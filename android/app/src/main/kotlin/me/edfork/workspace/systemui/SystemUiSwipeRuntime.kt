package me.edfork.workspace.systemui

import android.app.Application
import android.content.Context
import android.os.Handler
import android.os.Looper
import java.util.concurrent.Executors
import me.edfork.workspace.ipc.LocalDaemonClient
import me.edfork.workspace.liveupdate.FluidCloudBridge
import me.edfork.workspace.liveupdate.TransferFluidEvent
import me.edfork.workspace.liveupdate.TransferFluidTracker
import me.edfork.workspace.logging.WorkspaceLog
import me.edfork.workspace.model.SwipeDirection
import me.edfork.workspace.model.WorkspaceSnapshot

/**
 * Coordinates SystemUI gesture monitoring strictly according to live LAN connection state
 * streamed from `workspaced`, and drives ColorOS Fluid Cloud updates for clipboard & transfers.
 */
object SystemUiSwipeRuntime {
    private val worker = Executors.newSingleThreadExecutor { r ->
        Thread(r, "workspace-sysui-clip").apply { isDaemon = true }
    }
    private val transferTracker = TransferFluidTracker()

    @Volatile
    private var appContext: Context? = null

    @Volatile
    private var snapshot: WorkspaceSnapshot = WorkspaceSnapshot(daemonReachable = false)

    private val sseListener: (WorkspaceSnapshot) -> Unit = { snap ->
        snapshot = snap
        val ctx = appContext
        if (ctx != null) {
            transferTracker.onSnapshot(snap) { event ->
                dispatchTransferFluidEvent(ctx, event)
            }
            Handler(Looper.getMainLooper()).post {
                if (snap.isGestureActive) {
                    SystemUiGestureMonitor.install(ctx)
                } else {
                    SystemUiGestureMonitor.uninstall()
                }
            }
        }
    }

    fun currentSnapshot(): WorkspaceSnapshot = snapshot

    fun initialize(context: Context) {
        val app = (context.applicationContext ?: context)
        if (appContext != null) return
        appContext = app
        WorkspaceLog.info("sysui_runtime", "Initialized SystemUI LAN-gated runtime")
        LocalDaemonClient.addListener(sseListener)
        LocalDaemonClient.startSseMonitorIfNeeded()
    }

    fun shutdownForHotReload(): Application? {
        val saved = appContext as? Application
        LocalDaemonClient.removeListener(sseListener)
        transferTracker.reset()
        SystemUiGestureMonitor.uninstall()
        appContext = null
        snapshot = WorkspaceSnapshot(daemonReachable = false)
        return saved
    }

    fun onConfirmedSwipe(direction: SwipeDirection) {
        val snap = snapshot
        if (!snap.isGestureActive) return

        val action = when (direction) {
            SwipeDirection.UP -> snap.swipeUpAction
            SwipeDirection.DOWN -> snap.swipeDownAction
        }
        val peer = snap.onlinePairedPeers.firstOrNull()
        val ctx = appContext
        if (ctx != null) {
            FluidCloudBridge.showClipboardProgress(ctx, action, peer?.deviceName)
        }
        worker.execute {
            val res = LocalDaemonClient.triggerClipboardAction(action, peer?.deviceId)
            val currentCtx = appContext ?: return@execute
            if (res.ok) {
                FluidCloudBridge.showClipboardDone(
                    currentCtx,
                    action,
                    res.peerName.ifBlank { peer?.deviceName },
                    res.message,
                )
            } else {
                FluidCloudBridge.showClipboardError(currentCtx, action, res.message)
            }
        }
    }

    private fun dispatchTransferFluidEvent(ctx: Context, event: TransferFluidEvent) {
        val rec = event.record
        when (event) {
            is TransferFluidEvent.Progress -> FluidCloudBridge.showTransferProgress(
                context = ctx,
                direction = rec.direction,
                peerName = rec.peerName,
                summary = rec.summary,
                percent = event.percent,
                detail = event.detail,
                isMilestone = event.isFirstTick,
            )
            is TransferFluidEvent.Completed -> FluidCloudBridge.showTransferDone(
                context = ctx,
                direction = rec.direction,
                peerName = rec.peerName,
                summary = rec.summary,
                detail = event.detail,
            )
            is TransferFluidEvent.Cancelled -> FluidCloudBridge.showTransferCancelled(
                context = ctx,
                direction = rec.direction,
                peerName = rec.peerName,
                summary = rec.summary,
            )
            is TransferFluidEvent.Failed -> FluidCloudBridge.showTransferError(
                context = ctx,
                direction = rec.direction,
                peerName = rec.peerName,
                summary = rec.summary,
                errorMsg = event.errorMsg,
            )
        }
    }
}
