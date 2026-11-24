package me.edfork.workspace.liveupdate

import me.edfork.workspace.model.TransferRecord
import me.edfork.workspace.model.WorkspaceSnapshot

sealed interface TransferFluidEvent {
    val record: TransferRecord

    data class Progress(
        override val record: TransferRecord,
        val percent: Int,
        val detail: String,
        val isFirstTick: Boolean,
    ) : TransferFluidEvent

    data class Completed(
        override val record: TransferRecord,
        val detail: String,
    ) : TransferFluidEvent

    data class Cancelled(
        override val record: TransferRecord,
    ) : TransferFluidEvent

    data class Failed(
        override val record: TransferRecord,
        val errorMsg: String,
    ) : TransferFluidEvent
}

/**
 * Tracks SSE `transfers` deltas so active transfers publish throttled progress updates and
 * terminal states to Fluid Cloud once per transition without replaying historical records on connect.
 */
class TransferFluidTracker(
    private val progressThrottleMs: Long = 120L,
) {
    private data class SeenEntry(
        val state: String,
        val percent: Int,
        val summary: String,
        val lastEmitMs: Long,
    )

    private val lock = Any()
    private val seen = HashMap<String, SeenEntry>()
    private var initialized = false

    fun reset() {
        synchronized(lock) {
            seen.clear()
            initialized = false
        }
    }

    fun onSnapshot(
        snapshot: WorkspaceSnapshot,
        nowMs: Long = System.currentTimeMillis(),
        emit: (TransferFluidEvent) -> Unit,
    ) {
        val events = ArrayList<TransferFluidEvent>()
        synchronized(lock) {
            if (!snapshot.daemonReachable) {
                initialized = false
                seen.clear()
                return
            }

            if (!initialized) {
                initialized = true
                for (rec in snapshot.transfers) {
                    val normState = rec.state.lowercase()
                    val pct = FluidCloudFormatter.computePercent(rec.bytesTransferred, rec.totalBytes)
                    if (normState == "transferring") {
                        seen[rec.id] = SeenEntry(normState, pct, rec.summary, nowMs)
                        events.add(
                            TransferFluidEvent.Progress(
                                record = rec,
                                percent = pct,
                                detail = FluidCloudFormatter.formatTransferDetail(
                                    rec.summary,
                                    rec.bytesTransferred,
                                    rec.totalBytes,
                                    pct,
                                ),
                                isFirstTick = true,
                            ),
                        )
                    } else {
                        // Do not replay historical finished/failed records on initial connect.
                        seen[rec.id] = SeenEntry(normState, pct, rec.summary, nowMs)
                    }
                }
            } else {
                for (rec in snapshot.transfers) {
                    val normState = rec.state.lowercase()
                    val pct = FluidCloudFormatter.computePercent(rec.bytesTransferred, rec.totalBytes)
                    val prev = seen[rec.id]

                    when (normState) {
                        "transferring" -> {
                            val isNew = prev == null || prev.state != "transferring"
                            val changed = prev == null || prev.percent != pct || prev.summary != rec.summary
                            if (isNew || (changed && nowMs - prev.lastEmitMs >= progressThrottleMs)) {
                                seen[rec.id] = SeenEntry(normState, pct, rec.summary, nowMs)
                                events.add(
                                    TransferFluidEvent.Progress(
                                        record = rec,
                                        percent = pct,
                                        detail = FluidCloudFormatter.formatTransferDetail(
                                            rec.summary,
                                            rec.bytesTransferred,
                                            rec.totalBytes,
                                            pct,
                                        ),
                                        isFirstTick = isNew,
                                    ),
                                )
                            }
                        }
                        "completed" -> {
                            if (prev == null || prev.state != "completed") {
                                seen[rec.id] = SeenEntry(normState, 100, rec.summary, nowMs)
                                events.add(
                                    TransferFluidEvent.Completed(
                                        record = rec,
                                        detail = FluidCloudFormatter.formatTransferDetail(
                                            rec.summary,
                                            rec.bytesTransferred,
                                            rec.totalBytes,
                                            100,
                                        ),
                                    ),
                                )
                            }
                        }
                        "cancelled" -> {
                            if (prev == null || prev.state != "cancelled") {
                                seen[rec.id] = SeenEntry(normState, pct, rec.summary, nowMs)
                                events.add(TransferFluidEvent.Cancelled(record = rec))
                            }
                        }
                        "failed" -> {
                            if (prev == null || prev.state != "failed") {
                                seen[rec.id] = SeenEntry(normState, pct, rec.summary, nowMs)
                                events.add(
                                    TransferFluidEvent.Failed(
                                        record = rec,
                                        errorMsg = rec.errorMessage.orEmpty(),
                                    ),
                                )
                            }
                        }
                    }
                }
            }
        }

        for (event in events) {
            emit(event)
        }
    }
}
