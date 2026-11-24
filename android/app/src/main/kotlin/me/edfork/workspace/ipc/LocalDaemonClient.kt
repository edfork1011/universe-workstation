package me.edfork.workspace.ipc

import java.io.BufferedReader
import java.io.InputStream
import java.io.InputStreamReader
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import java.nio.charset.StandardCharsets
import java.util.concurrent.CopyOnWriteArraySet
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread
import me.edfork.workspace.logging.WorkspaceLog
import me.edfork.workspace.model.ClipboardActionResult
import me.edfork.workspace.model.MiniJson
import me.edfork.workspace.model.WorkspaceContract
import me.edfork.workspace.model.WorkspaceSnapshot
import me.edfork.workspace.model.bool
import me.edfork.workspace.model.numInt
import me.edfork.workspace.model.str

/**
 * Pure-JDK localhost HTTP/SSE client communicating with KernelSU `workspaced` (`127.0.0.1:45878`).
 * Zero external library dependencies; safe inside SystemUI, OShare, and app UI processes.
 */
object LocalDaemonClient {
    private val started = AtomicBoolean(false)
    private val listeners = CopyOnWriteArraySet<(WorkspaceSnapshot) -> Unit>()

    @Volatile
    var latestSnapshot: WorkspaceSnapshot = WorkspaceSnapshot(daemonReachable = false)
        private set

    fun startSseMonitorIfNeeded() {
        if (!started.compareAndSet(false, true)) return
        thread(isDaemon = true, name = "workspace-sse-client") {
            var backoffMs = 1000L
            while (started.get()) {
                val conn = runCatching {
                    (URL("${WorkspaceContract.LOCAL_IPC_BASE}/v1/local/events").openConnection() as HttpURLConnection).apply {
                        connectTimeout = 3000
                        readTimeout = 30_000
                        requestMethod = "GET"
                        setRequestProperty("Accept", "text/event-stream")
                    }
                }.getOrNull()

                if (conn == null) {
                    markOffline()
                    Thread.sleep(backoffMs)
                    backoffMs = (backoffMs * 2).coerceAtMost(5000L)
                    continue
                }

                try {
                    if (conn.responseCode in 200..299) {
                        backoffMs = 1000L
                        BufferedReader(InputStreamReader(conn.inputStream, StandardCharsets.UTF_8)).use { reader ->
                            while (started.get()) {
                                val line = reader.readLine() ?: break
                                if (line.startsWith("data:")) {
                                    val payload = line.removePrefix("data:").trim()
                                    if (payload.isNotEmpty()) {
                                        runCatching { WorkspaceSnapshot.fromJson(payload) }
                                            .onSuccess { snap -> updateSnapshot(snap) }
                                    }
                                }
                            }
                        }
                    } else {
                        markOffline()
                    }
                } catch (_: Throwable) {
                    markOffline()
                } finally {
                    runCatching { conn.disconnect() }
                }
                Thread.sleep(backoffMs)
                backoffMs = (backoffMs * 2).coerceAtMost(5000L)
            }
        }
    }

    fun stopSseMonitor() {
        started.set(false)
        listeners.clear()
        markOffline()
    }

    fun addListener(listener: (WorkspaceSnapshot) -> Unit) {
        listeners.add(listener)
        listener(latestSnapshot)
    }

    fun removeListener(listener: (WorkspaceSnapshot) -> Unit) {
        listeners.remove(listener)
    }

    private fun markOffline() {
        val prev = latestSnapshot
        if (prev.daemonReachable || prev.lanConnected) {
            updateSnapshot(WorkspaceSnapshot(daemonReachable = false))
        }
    }

    private fun updateSnapshot(snap: WorkspaceSnapshot) {
        latestSnapshot = snap
        for (listener in listeners) {
            runCatching { listener(snap) }
        }
    }

    fun fetchStatusSync(): Result<WorkspaceSnapshot> = runCatching {
        val raw = requestJson("GET", "/v1/local/status", null)
        WorkspaceSnapshot.fromJson(raw).also { updateSnapshot(it) }
    }

    fun triggerClipboardAction(action: String, peerId: String? = null): ClipboardActionResult {
        val path = when (action.lowercase()) {
            "push" -> "/v1/local/clipboard/push"
            "pull" -> "/v1/local/clipboard/pull"
            else -> return ClipboardActionResult(false, "未知动作: $action")
        }
        val bodyMap = LinkedHashMap<String, Any?>()
        if (!peerId.isNullOrEmpty()) bodyMap["peer_id"] = peerId
        val body = MiniJson.stringifyObject(bodyMap)

        return runCatching {
            val raw = requestJson("POST", path, body)
            val obj = MiniJson.parseObject(raw)
            ClipboardActionResult(
                ok = obj.bool("ok"),
                message = obj.str("message", "已完成"),
                peerName = obj.str("peer_name", ""),
                size = obj.numInt("size", 0),
            )
        }.getOrElse { err ->
            WorkspaceLog.warn("local_ipc", "clipboard action `$action` failed", err)
            ClipboardActionResult(false, err.message ?: "剪贴板同步失败")
        }
    }

    fun startPair(deviceId: String): Result<Unit> = runCatching {
        requestJson("POST", "/v1/local/pair/start", MiniJson.stringifyObject(mapOf("device_id" to deviceId)))
        fetchStatusSync()
    }

    fun approvePair(sessionId: String): Result<Unit> = runCatching {
        requestJson("POST", "/v1/local/pair/approve", MiniJson.stringifyObject(mapOf("session_id" to sessionId)))
        fetchStatusSync()
    }

    fun rejectPair(sessionId: String): Result<Unit> = runCatching {
        requestJson("POST", "/v1/local/pair/reject", MiniJson.stringifyObject(mapOf("session_id" to sessionId)))
        fetchStatusSync()
    }

    fun unpair(deviceId: String): Result<Unit> = runCatching {
        requestJson("POST", "/v1/local/unpair", MiniJson.stringifyObject(mapOf("device_id" to deviceId)))
        fetchStatusSync()
    }

    fun updateConfig(
        gestureEnabled: Boolean? = null,
        syncPaused: Boolean? = null,
        swipeUpAction: String? = null,
        swipeDownAction: String? = null,
        deviceName: String? = null,
        userName: String? = null,
        systemDeviceName: String? = null,
    ): Result<Unit> = runCatching {
        val map = LinkedHashMap<String, Any?>()
        gestureEnabled?.let { map["gesture_enabled"] = it }
        syncPaused?.let { map["sync_paused"] = it }
        swipeUpAction?.let { map["swipe_up_action"] = it }
        swipeDownAction?.let { map["swipe_down_action"] = it }
        deviceName?.let { map["device_name"] = it }
        userName?.let { map["user_name"] = it }
        systemDeviceName?.let { map["system_device_name"] = it }
        requestJson("POST", "/v1/local/config", MiniJson.stringifyObject(map))
        fetchStatusSync()
    }

    fun streamOShareFile(
        peerId: String,
        batchId: String,
        fileName: String,
        totalSize: Long,
        fileIndex: Int,
        fileTotal: Int,
        inputStream: InputStream,
        onBytesSent: (Long) -> Unit,
    ): Result<Unit> = runCatching {
        val encodedName = URLEncoder.encode(fileName, "UTF-8").replace("+", "%20")
        val conn = (URL("${WorkspaceContract.LOCAL_IPC_BASE}/v1/local/oshare/send").openConnection() as HttpURLConnection).apply {
            connectTimeout = 5000
            readTimeout = 300_000
            requestMethod = "POST"
            doOutput = true
            setRequestProperty("X-Workspace-Peer-Id", peerId)
            setRequestProperty("X-Workspace-Batch-Id", batchId)
            setRequestProperty("X-Workspace-File-Name", encodedName)
            setRequestProperty("X-Workspace-File-Size", totalSize.toString())
            setRequestProperty("X-Workspace-File-Index", fileIndex.toString())
            setRequestProperty("X-Workspace-File-Total", fileTotal.toString())
            if (totalSize >= 0L) {
                setFixedLengthStreamingMode(totalSize)
            } else {
                setChunkedStreamingMode(64 * 1024)
            }
        }
        try {
            val buf = ByteArray(64 * 1024)
            var sent = 0L
            conn.outputStream.use { out ->
                while (true) {
                    val n = inputStream.read(buf)
                    if (n < 0) break
                    out.write(buf, 0, n)
                    sent += n
                    onBytesSent(sent)
                }
                out.flush()
            }
            val status = conn.responseCode
            val errOrOk = if (status in 200..299) {
                conn.inputStream.bufferedReader().readText()
            } else {
                val errBody = conn.errorStream?.bufferedReader()?.readText().orEmpty()
                error("HTTP $status: $errBody")
            }
            WorkspaceLog.info("oshare_send", "sent $fileName ($fileIndex/$fileTotal) -> $errOrOk")
        } finally {
            runCatching { conn.disconnect() }
        }
    }

    fun cancelOShare(batchOrPeerId: String) {
        runCatching {
            requestJson("POST", "/v1/local/oshare/cancel", MiniJson.stringifyObject(mapOf("id" to batchOrPeerId)))
        }
    }

    private fun requestJson(method: String, path: String, jsonBody: String?): String {
        val conn = (URL("${WorkspaceContract.LOCAL_IPC_BASE}$path").openConnection() as HttpURLConnection).apply {
            connectTimeout = 4000
            readTimeout = 12_000
            requestMethod = method
            if (jsonBody != null) {
                doOutput = true
                setRequestProperty("Content-Type", "application/json")
            }
        }
        try {
            if (jsonBody != null) {
                conn.outputStream.use { it.write(jsonBody.toByteArray(StandardCharsets.UTF_8)) }
            }
            val code = conn.responseCode
            val stream = if (code in 200..299) conn.inputStream else conn.errorStream
            val bodyText = stream?.bufferedReader(StandardCharsets.UTF_8)?.use { it.readText() }.orEmpty()
            if (code !in 200..299) {
                val parsedErr = runCatching {
                    val obj = MiniJson.parseObject(bodyText)
                    (obj["error"] as? Map<*, *>)?.get("message") as? String
                }.getOrNull()
                error(parsedErr?.takeIf { it.isNotBlank() } ?: "HTTP $code: $bodyText")
            }
            return bodyText
        } finally {
            conn.disconnect()
        }
    }
}
