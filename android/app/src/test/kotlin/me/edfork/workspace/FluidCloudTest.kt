package me.edfork.workspace

import me.edfork.workspace.liveupdate.FluidAction
import me.edfork.workspace.liveupdate.FluidCloudController
import me.edfork.workspace.liveupdate.FluidCloudFormatter
import me.edfork.workspace.liveupdate.FluidPhase
import me.edfork.workspace.liveupdate.TransferFluidEvent
import me.edfork.workspace.liveupdate.TransferFluidTracker
import me.edfork.workspace.model.TransferRecord
import me.edfork.workspace.model.WorkspaceSnapshot
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FluidCloudTest {

    @Test
    fun formatterProducesExpectedCapsuleTextAndPercentages() {
        assertEquals(-1, FluidCloudFormatter.computePercent(100L, 0L))
        assertEquals(-1, FluidCloudFormatter.computePercent(100L, -1L))
        assertEquals(0, FluidCloudFormatter.computePercent(0L, 1000L))
        assertEquals(50, FluidCloudFormatter.computePercent(500L, 1000L))
        assertEquals(99, FluidCloudFormatter.computePercent(1000L, 1000L))

        assertEquals("推送中", FluidCloudFormatter.capsuleTextFor(FluidAction.CLIP_PUSH, FluidPhase.PROGRESS))
        assertEquals("已推送", FluidCloudFormatter.capsuleTextFor(FluidAction.CLIP_PUSH, FluidPhase.DONE))
        assertEquals("推送失败", FluidCloudFormatter.capsuleTextFor(FluidAction.CLIP_PUSH, FluidPhase.ERROR))

        assertEquals("拉取中", FluidCloudFormatter.capsuleTextFor(FluidAction.CLIP_PULL, FluidPhase.PROGRESS))
        assertEquals("已拉取", FluidCloudFormatter.capsuleTextFor(FluidAction.CLIP_PULL, FluidPhase.DONE))
        assertEquals("拉取失败", FluidCloudFormatter.capsuleTextFor(FluidAction.CLIP_PULL, FluidPhase.ERROR))

        assertEquals("发送中", FluidCloudFormatter.capsuleTextFor(FluidAction.FILE_SEND, FluidPhase.PROGRESS, 0))
        assertEquals("发送 42%", FluidCloudFormatter.capsuleTextFor(FluidAction.FILE_SEND, FluidPhase.PROGRESS, 42))
        assertEquals("已发送", FluidCloudFormatter.capsuleTextFor(FluidAction.FILE_SEND, FluidPhase.DONE))
        assertEquals("已取消", FluidCloudFormatter.capsuleTextFor(FluidAction.FILE_SEND, FluidPhase.CANCELLED))
        assertEquals("发送失败", FluidCloudFormatter.capsuleTextFor(FluidAction.FILE_SEND, FluidPhase.ERROR))

        assertEquals("接收中", FluidCloudFormatter.capsuleTextFor(FluidAction.FILE_RECEIVE, FluidPhase.PROGRESS, -1))
        assertEquals("接收 78%", FluidCloudFormatter.capsuleTextFor(FluidAction.FILE_RECEIVE, FluidPhase.PROGRESS, 78))
        assertEquals("已接收", FluidCloudFormatter.capsuleTextFor(FluidAction.FILE_RECEIVE, FluidPhase.DONE))
        assertEquals("接收失败", FluidCloudFormatter.capsuleTextFor(FluidAction.FILE_RECEIVE, FluidPhase.ERROR))

        assertEquals(5000L, FluidCloudController.CLIP_DONE_DISMISS_MS)
        assertEquals(5000L, FluidCloudController.CLIP_ERROR_DISMISS_MS)
        assertEquals(5000L, FluidCloudController.TRANSFER_DONE_DISMISS_MS)
        assertEquals(5000L, FluidCloudController.TRANSFER_ERROR_DISMISS_MS)
    }

    @Test
    fun transferTrackerSkipsHistoricalCompletedAndThrottlesProgress() {
        val tracker = TransferFluidTracker(progressThrottleMs = 120L)
        val events = mutableListOf<TransferFluidEvent>()

        val historical = transfer("old-1", "send", "completed", 1024L, 1024L)
        tracker.onSnapshot(snap(true, listOf(historical)), nowMs = 1000L) { events.add(it) }
        assertTrue("Historical finished transfer must not replay on connect", events.isEmpty())

        // New active transfer begins
        val tStart = transfer("tx-1", "send", "transferring", 0L, 1000L)
        tracker.onSnapshot(snap(true, listOf(tStart, historical)), nowMs = 1100L) { events.add(it) }
        assertEquals(1, events.size)
        val first = events[0] as TransferFluidEvent.Progress
        assertTrue(first.isFirstTick)
        assertEquals(0, first.percent)

        // Rapid progress within throttle window -> suppressed
        events.clear()
        val tFast = transfer("tx-1", "send", "transferring", 200L, 1000L)
        tracker.onSnapshot(snap(true, listOf(tFast, historical)), nowMs = 1150L) { events.add(it) }
        assertTrue(events.isEmpty())

        // Progress after throttle window -> emitted with milestone=false (isFirstTick=false)
        val tMid = transfer("tx-1", "send", "transferring", 650L, 1000L)
        tracker.onSnapshot(snap(true, listOf(tMid, historical)), nowMs = 1250L) { events.add(it) }
        assertEquals(1, events.size)
        val mid = events[0] as TransferFluidEvent.Progress
        assertEquals(false, mid.isFirstTick)
        assertEquals(65, mid.percent)

        // Terminal transition happens immediately without throttle delay
        events.clear()
        val tDone = transfer("tx-1", "send", "completed", 1000L, 1000L)
        tracker.onSnapshot(snap(true, listOf(tDone, historical)), nowMs = 1260L) { events.add(it) }
        assertEquals(1, events.size)
        assertTrue(events[0] is TransferFluidEvent.Completed)

        // Duplicate completed snapshot does not re-emit
        events.clear()
        tracker.onSnapshot(snap(true, listOf(tDone, historical)), nowMs = 1400L) { events.add(it) }
        assertTrue(events.isEmpty())
    }

    @Test
    fun transferTrackerHandlesSmallInstantCompletedCancelledFailedAndReconnect() {
        val tracker = TransferFluidTracker(progressThrottleMs = 120L)
        val events = mutableListOf<TransferFluidEvent>()

        // Initialize online with empty list
        tracker.onSnapshot(snap(true, emptyList()), nowMs = 100L) { events.add(it) }
        assertTrue(events.isEmpty())

        // Tiny file completes in a single poll without an observed transferring intermediate state
        val tinyRecv = transfer("rx-tiny", "receive", "completed", 512L, 512L)
        tracker.onSnapshot(snap(true, listOf(tinyRecv)), nowMs = 200L) { events.add(it) }
        assertEquals(1, events.size)
        assertTrue(events[0] is TransferFluidEvent.Completed)

        // Cancelled & Failed states
        events.clear()
        val cancelled = transfer("tx-cancel", "send", "cancelled", 100L, 1000L)
        val failed = transfer("rx-fail", "receive", "failed", 10L, 1000L, error = "connection reset")
        tracker.onSnapshot(snap(true, listOf(failed, cancelled, tinyRecv)), nowMs = 300L) { events.add(it) }
        assertEquals(2, events.size)
        assertTrue(events.any { it is TransferFluidEvent.Failed && it.errorMsg == "connection reset" })
        assertTrue(events.any { it is TransferFluidEvent.Cancelled })

        // Disconnect + reconnect must not replay those records
        events.clear()
        tracker.onSnapshot(snap(false, emptyList()), nowMs = 400L) { events.add(it) }
        tracker.onSnapshot(snap(true, listOf(failed, cancelled, tinyRecv)), nowMs = 500L) { events.add(it) }
        assertTrue(events.isEmpty())
    }

    private fun snap(reachable: Boolean, transfers: List<TransferRecord>): WorkspaceSnapshot =
        WorkspaceSnapshot(daemonReachable = reachable, transfers = transfers)

    private fun transfer(
        id: String,
        direction: String,
        state: String,
        bytes: Long,
        total: Long,
        error: String? = null,
    ): TransferRecord = TransferRecord(
        id = id,
        peerId = "linux-peer-1",
        peerName = "EDFORK-LINUXPC",
        direction = direction,
        summary = "sample.bin",
        fileCount = 1,
        bytesTransferred = bytes,
        totalBytes = total,
        state = state,
        errorMessage = error,
    )
}
