package me.edfork.workspace.model

object WorkspaceContract {
    const val SYSTEM_UI_PACKAGE = "com.android.systemui"
    const val OSHARE_PACKAGE = "com.coloros.oshare"
    const val LOCAL_IPC_BASE = "http://127.0.0.1:45878"
    const val OSHARE_MAC_PREFIX = "workspace_lan:"

    fun oshareBleMac(deviceId: String): String = "$OSHARE_MAC_PREFIX$deviceId"

    fun parseDeviceIdFromOshareMac(bleMac: String?): String? {
        if (bleMac == null || !bleMac.startsWith(OSHARE_MAC_PREFIX)) return null
        val id = bleMac.removePrefix(OSHARE_MAC_PREFIX)
        return id.takeIf { it.isNotEmpty() }
    }
}

enum class SwipeDirection {
    UP,
    DOWN,
}

data class PairedPeerStatus(
    val deviceId: String,
    val deviceName: String,
    val platform: String,
    val fingerprint: String,
    val connected: Boolean,
    val ip: String?,
    val tlsPort: Int,
    val rttMs: Long?,
)

data class DiscoveredPeer(
    val deviceId: String,
    val deviceName: String,
    val platform: String,
    val ip: String,
    val tlsPort: Int,
    val fingerprint: String,
    val isTrusted: Boolean,
)

data class PendingPairView(
    val sessionId: String,
    val peerId: String,
    val peerName: String,
    val peerFingerprint: String,
    val sasCode: String,
    val isInitiator: Boolean,
    val state: String,
)

data class ClipboardSummary(
    val direction: String,
    val peerName: String,
    val kind: String,
    val size: Int,
    val timestampMs: Long,
)

data class TransferRecord(
    val id: String,
    val peerId: String,
    val peerName: String,
    val direction: String,
    val summary: String,
    val fileCount: Int = 1,
    val bytesTransferred: Long,
    val totalBytes: Long,
    val state: String,
    val errorMessage: String?,
    val destPath: String? = null,
    val updatedAtMs: Long = 0L,
)

data class WorkspaceSnapshot(
    val daemonReachable: Boolean = false,
    val deviceId: String = "",
    val deviceName: String = "",
    val deviceNameCustomized: Boolean = false,
    val userName: String = "User",
    val fingerprint: String = "",
    val tlsPort: Int = 45877,
    val lanConnected: Boolean = false,
    val syncPaused: Boolean = false,
    val gestureEnabled: Boolean = true,
    val gestureFingers: Int = 2,
    val swipeUpAction: String = "push",
    val swipeDownAction: String = "pull",
    val receiveDir: String = "/storage/emulated/0/Download/寰宇工作台",
    val pairedPeers: List<PairedPeerStatus> = emptyList(),
    val discoveredPeers: List<DiscoveredPeer> = emptyList(),
    val pendingPairs: List<PendingPairView> = emptyList(),
    val lastClipboard: ClipboardSummary? = null,
    val transfers: List<TransferRecord> = emptyList(),
) {
    /** Hard gate required by spec: gestures are active ONLY when LAN is connected to a paired PC. */
    val isGestureActive: Boolean
        get() = daemonReachable && lanConnected && gestureEnabled && !syncPaused

    val onlinePairedPeers: List<PairedPeerStatus>
        get() = if (daemonReachable) pairedPeers.filter { it.connected } else emptyList()

    companion object {
        fun fromJson(raw: String): WorkspaceSnapshot {
            val root = MiniJson.parseObject(raw)
            val paired = root.objList("paired_peers").map { o ->
                PairedPeerStatus(
                    deviceId = o.str("device_id"),
                    deviceName = o.str("device_name"),
                    platform = o.str("platform"),
                    fingerprint = o.str("fingerprint"),
                    connected = o.bool("connected"),
                    ip = o.optStr("ip"),
                    tlsPort = o.numInt("tls_port", 45877),
                    rttMs = o.optLong("rtt_ms"),
                )
            }
            val discovered = root.objList("discovered_peers").map { o ->
                DiscoveredPeer(
                    deviceId = o.str("device_id"),
                    deviceName = o.str("device_name"),
                    platform = o.str("platform"),
                    ip = o.str("ip"),
                    tlsPort = o.numInt("tls_port", 45877),
                    fingerprint = o.str("fingerprint"),
                    isTrusted = o.bool("is_trusted"),
                )
            }
            val pending = root.objList("pending_pairs").map { o ->
                PendingPairView(
                    sessionId = o.str("session_id"),
                    peerId = o.str("peer_id"),
                    peerName = o.str("peer_name"),
                    peerFingerprint = o.str("peer_fingerprint"),
                    sasCode = o.str("sas_code"),
                    isInitiator = o.bool("is_initiator"),
                    state = o.str("state", "pending"),
                )
            }
            val transfers = root.objList("transfers").map { o ->
                TransferRecord(
                    id = o.str("id"),
                    peerId = o.str("peer_id"),
                    peerName = o.str("peer_name"),
                    direction = o.str("direction"),
                    summary = o.str("summary"),
                    fileCount = o.numInt("file_count", 1),
                    bytesTransferred = o.numLong("bytes_transferred", 0L),
                    totalBytes = o.numLong("total_bytes", 0L),
                    state = o.str("state"),
                    errorMessage = o.optStr("error_message"),
                    destPath = o.optStr("dest_path"),
                    updatedAtMs = o.numLong("updated_at_ms", 0L),
                )
            }
            val lastClip = root.obj("last_clipboard")?.let { c ->
                ClipboardSummary(
                    direction = c.str("direction"),
                    peerName = c.str("peer_name"),
                    kind = c.str("kind"),
                    size = c.numInt("size"),
                    timestampMs = c.numLong("timestamp_ms"),
                )
            }

            return WorkspaceSnapshot(
                daemonReachable = true,
                deviceId = root.str("device_id"),
                deviceName = root.str("device_name"),
                deviceNameCustomized = root.bool("device_name_customized", false),
                userName = root.str("user_name", "User").ifBlank { "User" },
                fingerprint = root.str("fingerprint"),
                tlsPort = root.numInt("tls_port", 45877),
                lanConnected = root.bool("lan_connected"),
                syncPaused = root.bool("sync_paused"),
                gestureEnabled = root.bool("gesture_enabled", true),
                gestureFingers = root.numInt("gesture_fingers", 2),
                swipeUpAction = root.str("swipe_up_action", "push"),
                swipeDownAction = root.str("swipe_down_action", "pull"),
                receiveDir = root.str("receive_dir", "/storage/emulated/0/Download/寰宇工作台"),
                pairedPeers = paired,
                discoveredPeers = discovered,
                pendingPairs = pending,
                lastClipboard = lastClip,
                transfers = transfers,
            )
        }
    }
}

data class ClipboardActionResult(
    val ok: Boolean,
    val message: String,
    val peerName: String = "",
    val size: Int = 0,
)
