package me.edfork.workspace.oshare

import android.app.Application
import android.content.ClipData
import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.Parcelable
import android.os.SystemClock
import android.provider.OpenableColumns
import android.util.SparseArray
import java.io.ByteArrayInputStream
import java.io.File
import java.io.FileInputStream
import java.nio.charset.StandardCharsets
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import me.edfork.workspace.ipc.LocalDaemonClient
import me.edfork.workspace.liveupdate.FluidCloudBridge
import me.edfork.workspace.logging.WorkspaceLog
import me.edfork.workspace.model.PairedPeerStatus
import me.edfork.workspace.model.WorkspaceContract
import me.edfork.workspace.model.WorkspaceSnapshot

/**
 * Manages injection of paired online LAN PCs into ColorOS `OShare` (`v9.a` DeviceManager +
 * `DirectShareServer`) and executes click-to-send transfers strictly over LAN mTLS via `workspaced`.
 */
object OShareLanRuntime {
    private val sendExecutor = Executors.newSingleThreadExecutor { r ->
        Thread(r, "workspace-oshare-lan-send").apply { isDaemon = true }
    }
    private val fetchExecutor = Executors.newSingleThreadExecutor { r ->
        Thread(r, "workspace-oshare-status-fetch").apply { isDaemon = true }
    }
    private val fetchInFlight = AtomicBoolean(false)
    private val activeCancelled = ConcurrentHashMap<String, AtomicBoolean>()

    @Volatile
    private var appContext: Context? = null

    @Volatile
    private var appClassLoader: ClassLoader? = null

    @Volatile
    private var snapshot: WorkspaceSnapshot = WorkspaceSnapshot(daemonReachable = false)

    @Volatile
    private var pcBitmapCache: Bitmap? = null

    private val sseListener: (WorkspaceSnapshot) -> Unit = { snap ->
        snapshot = snap
        syncVirtualDevicesToManager()
    }

    fun initialize(context: Context, classLoader: ClassLoader) {
        appClassLoader = classLoader
        val firstInit = appContext == null
        if (firstInit) {
            appContext = context.applicationContext ?: context
            WorkspaceLog.info("oshare_runtime", "Initialized OShare LAN integration runtime")
            LocalDaemonClient.addListener(sseListener)
        }
        LocalDaemonClient.startSseMonitorIfNeeded()
        triggerBackgroundRefresh()
        syncVirtualDevicesToManager()
    }

    fun shutdownForHotReload(): Application? {
        LocalDaemonClient.removeListener(sseListener)
        val prevApp = appContext as? Application
        appContext = null
        appClassLoader = null
        return prevApp
    }

    private fun triggerBackgroundRefresh() {
        if (!fetchInFlight.compareAndSet(false, true)) return
        fetchExecutor.execute {
            try {
                LocalDaemonClient.fetchStatusSync().onSuccess { snap ->
                    snapshot = snap
                    syncVirtualDevicesToManager()
                }
            } finally {
                fetchInFlight.set(false)
            }
        }
    }

    fun hasOnlinePeer(): Boolean = snapshot.onlinePairedPeers.isNotEmpty()

    fun isPeerOnline(bleMac: String?): Boolean {
        val id = WorkspaceContract.parseDeviceIdFromOshareMac(bleMac) ?: return false
        return snapshot.onlinePairedPeers.any { it.deviceId == id }
    }

    fun currentOnlineDeviceList(deviceList: List<*>?): Boolean {
        if (deviceList == null) return false
        return deviceList.any { item ->
            val mac = invokeNoArg(item, "a") as? String
            mac?.startsWith(WorkspaceContract.OSHARE_MAC_PREFIX) == true
        }
    }

    @Suppress("UNCHECKED_CAST")
    private fun resolveDeviceMap(dmClass: Class<*>, dm: Any): MutableMap<String, Any>? {
        val field = runCatching { dmClass.getDeclaredField("a") }
            .getOrElse { dmClass.getDeclaredField("f24124a") }
        field.isAccessible = true
        return field.get(dm) as? MutableMap<String, Any>
    }

    private fun upsertOnlineDevices(cl: ClassLoader, dmClass: Class<*>, dm: Any): Boolean {
        val map = resolveDeviceMap(dmClass, dm) ?: return false
        val online = snapshot.onlinePairedPeers
        val wantedKeys = online.map { WorkspaceContract.oshareBleMac(it.deviceId) }.toSet()
        var changed = false

        // Remove offline Workspace LAN entries immediately
        val toRemove = map.keys.filter {
            it.startsWith(WorkspaceContract.OSHARE_MAC_PREFIX) && !wantedKeys.contains(it)
        }
        for (k in toRemove) {
            if (map.remove(k) != null) {
                changed = true
            }
        }

        // Insert or refresh online paired PCs and keep their virtual TTL alive
        val nowMs = System.currentTimeMillis()
        for (peer in online) {
            val key = WorkspaceContract.oshareBleMac(peer.deviceId)
            val wantSub = subtitleForPeer(peer)
            val existing = map[key]
            if (existing != null) {
                val curName = invokeNoArg(existing, "j") as? String
                val curSub = invokeNoArg(existing, "d0") as? String
                if (curName != peer.deviceName) {
                    invokeOneArg(existing, "F", peer.deviceName)
                    changed = true
                }
                if (curSub != wantSub) {
                    invokeOneArg(existing, "G0", wantSub)
                    changed = true
                }
                invokeOneArg(existing, "O", 8)
                invokeOneArg(existing, "E", nowMs.toString())
            } else {
                createVirtualPcDevice(cl, peer)?.let { dev ->
                    map[key] = dev
                    changed = true
                }
            }
        }
        return changed
    }

    fun syncVirtualDevicesToManager() {
        val cl = appClassLoader ?: return
        runCatching {
            val dmClass = cl.loadClass("v9.a")
            val dm = dmClass.getMethod("h").invoke(null) ?: return
            upsertOnlineDevices(cl, dmClass, dm)
            Handler(Looper.getMainLooper()).post {
                runCatching { dmClass.getMethod("r").invoke(dm) }
            }
        }.onFailure { err ->
            WorkspaceLog.warn("oshare_runtime", "Failed syncing virtual PC devices to v9.a", err)
        }
    }

    fun refreshVirtualDevicesTtl() {
        val cl = appClassLoader ?: return
        runCatching {
            val dmClass = cl.loadClass("v9.a")
            val dm = dmClass.getMethod("h").invoke(null) ?: return
            val changed = upsertOnlineDevices(cl, dmClass, dm)
            if (changed) {
                Handler(Looper.getMainLooper()).post {
                    runCatching { dmClass.getMethod("r").invoke(dm) }
                }
            }
        }.onFailure { err ->
            WorkspaceLog.warn("oshare_runtime", "Failed refreshing virtual PC TTL in v9.a", err)
        }
    }

    fun ensureSyncedForDeviceQuery() {
        val cl = appClassLoader ?: return
        if (!snapshot.daemonReachable) {
            if (Looper.myLooper() != Looper.getMainLooper()) {
                LocalDaemonClient.fetchStatusSync().onSuccess { snap ->
                    snapshot = snap
                }
            } else {
                triggerBackgroundRefresh()
            }
        }
        runCatching {
            val dmClass = cl.loadClass("v9.a")
            val dm = dmClass.getMethod("h").invoke(null) ?: return
            upsertOnlineDevices(cl, dmClass, dm)
        }.onFailure { err ->
            WorkspaceLog.warn("oshare_runtime", "Failed ensuring virtual PC sync on query", err)
        }
    }

    private fun subtitleForPeer(peer: PairedPeerStatus): String =
        peer.ip?.let { "寰宇工作台 · $it" } ?: "寰宇工作台 · 局域网"

    private fun createVirtualPcDevice(cl: ClassLoader, peer: PairedPeerStatus): Any? {
        val devClass = runCatching { cl.loadClass("v9.d") }
            .getOrElse { cl.loadClass("l9.b") }
        val dev = devClass.getConstructor().newInstance()
        val mac = WorkspaceContract.oshareBleMac(peer.deviceId)
        val icon = getOrCreatePcBitmap()
        val nowMs = System.currentTimeMillis()

        invokeOneArg(dev, "w", mac)
        invokeOneArg(dev, "P", "02:00:00:00:00:01")
        invokeOneArg(dev, "F", peer.deviceName)
        invokeOneArg(dev, "w0", 5)
        invokeOneArg(dev, "M", 802)
        invokeOneArg(dev, "O", 8)
        invokeOneArg(dev, "G0", subtitleForPeer(peer))
        invokeOneArg(dev, "D0", true)
        invokeOneArg(dev, "A", icon)
        invokeOneArg(dev, "t0", icon)
        invokeOneArg(dev, "D", nowMs)
        invokeOneArg(dev, "E", nowMs.toString())
        invokeOneArg(dev, "H", 0)
        invokeOneArg(dev, "J", 0)
        invokeOneArg(dev, "K", 1)

        val idleState = enumConstant(cl, "com.oplus.oshare.OplusOshareState", "IDLE")
        if (idleState != null) {
            invokeOneArg(dev, "I", idleState)
        }
        return dev
    }

    private fun getOrCreatePcBitmap(): Bitmap {
        pcBitmapCache?.let { return it }
        val size = 128
        val bmp = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bmp)

        val bgPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = 0xFF006A60.toInt() // Material 3 Teal Primary
            style = Paint.Style.FILL
        }
        canvas.drawCircle(size / 2f, size / 2f, size * 0.46f, bgPaint)

        val strokePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = 0xFFFFFFFF.toInt()
            style = Paint.Style.STROKE
            strokeWidth = 6f
            strokeCap = Paint.Cap.ROUND
            strokeJoin = Paint.Join.ROUND
        }
        // Laptop screen + base silhouette
        canvas.drawRoundRect(RectF(32f, 38f, 96f, 78f), 6f, 6f, strokePaint)
        canvas.drawLine(24f, 88f, 104f, 88f, strokePaint)

        pcBitmapCache = bmp
        return bmp
    }

    fun dispatchWhenBtOffIfNeeded(
        directShareServer: Any,
        sparseArray: SparseArray<*>,
        deviceList: List<*>,
    ) {
        val cl = appClassLoader ?: return
        runCatching {
            val btMgrClass = cl.loadClass("com.oplus.oshare.utils.j")
            val inst = runCatching { btMgrClass.getMethod("c").invoke(null) }
                .getOrElse {
                    val companion = runCatching { btMgrClass.getField("a") }
                        .getOrElse { btMgrClass.getField("f12100a") }
                        .get(null)
                    companion.javaClass.getMethod("a").invoke(companion)
                }
            val btOn = inst.javaClass.getMethod("h").invoke(inst) as? Boolean ?: true
            if (btOn) return

            if (!currentOnlineDeviceList(deviceList)) return

            val directCls = directShareServer.javaClass
            val iMap = directCls.getMethod("I").invoke(directShareServer) as? Map<*, *>
            val snapshotPairs = synchronized(sparseArray) {
                (0 until sparseArray.size()).map { idx ->
                    sparseArray.keyAt(idx) to sparseArray.valueAt(idx)
                }
            }

            for ((key, listener) in snapshotPairs) {
                if (listener == null || iMap?.containsKey(key) == true) continue
                runCatching {
                    listener.javaClass.getMethod("c", List::class.java).invoke(listener, deviceList)
                }
            }
        }
    }

    fun startLanSend(intent: Intent, targetDevice: Any) {
        val ctx = appContext ?: return
        val cl = appClassLoader ?: return
        val bleMac = invokeNoArg(targetDevice, "a") as? String ?: return
        val peerId = WorkspaceContract.parseDeviceIdFromOshareMac(bleMac) ?: return
        val peerName = snapshot.onlinePairedPeers.firstOrNull { it.deviceId == peerId }?.deviceName ?: "电脑"

        val cancelFlag = AtomicBoolean(false)
        activeCancelled[bleMac] = cancelFlag

        val items = extractSendItems(ctx, intent, cl)
        if (items.isEmpty()) {
            WorkspaceLog.warn("oshare_send", "No transferable items extracted from Share Intent")
            updateDeviceProgress(cl, bleMac, "TRANSIT_FAILED", 0, 0, 1)
            FluidCloudBridge.showTransferError(ctx, "send", peerName, "OPPO 分享", "未检测到可传输的文件或文本")
            scheduleResetIdle(cl, bleMac)
            return
        }

        val batchId = UUID.randomUUID().toString()
        sendExecutor.execute {
            val cr = ctx.contentResolver
            val totalFiles = items.size
            val totalBytes = items.sumOf { it.size.coerceAtLeast(0L) }
            var completedPrefixBytes = 0L
            var succeeded = 0
            var streamAttempted = false

            updateDeviceProgress(cl, bleMac, "TRANSITING", 0, 0, totalFiles)

            try {
                for ((idx, item) in items.withIndex()) {
                    if (cancelFlag.get()) {
                        updateDeviceProgress(cl, bleMac, "CANCEL", 0, succeeded, totalFiles)
                        scheduleResetIdle(cl, bleMac)
                        return@execute
                    }

                    val streamRes = item.openStream(cr).use { input ->
                        streamAttempted = true
                        var lastNotifyMs = 0L
                        LocalDaemonClient.streamOShareFile(
                            peerId = peerId,
                            batchId = batchId,
                            fileName = item.fileName,
                            totalSize = item.size,
                            fileIndex = idx + 1,
                            fileTotal = totalFiles,
                            inputStream = input,
                        ) { sentInFile ->
                            if (cancelFlag.get()) {
                                throw InterruptedException("cancelled")
                            }
                            val now = SystemClock.elapsedRealtime()
                            if (now - lastNotifyMs >= 80L) {
                                lastNotifyMs = now
                                val overallPct = if (totalBytes > 0L) {
                                    (((completedPrefixBytes + sentInFile) * 100L) / totalBytes)
                                        .toInt()
                                        .coerceIn(1, 99)
                                } else {
                                    (((idx * 100) + 50) / totalFiles).coerceIn(1, 99)
                                }
                                updateDeviceProgress(cl, bleMac, "TRANSITING", overallPct, succeeded, totalFiles)
                            }
                        }
                    }

                    if (streamRes.isFailure) {
                        throw streamRes.exceptionOrNull() ?: RuntimeException("LAN upload failed")
                    }
                    completedPrefixBytes += item.size.coerceAtLeast(0L)
                    succeeded += 1
                    val pct = ((succeeded * 100) / totalFiles).coerceIn(1, 100)
                    updateDeviceProgress(cl, bleMac, "TRANSITING", pct, succeeded, totalFiles)
                }

                updateDeviceProgress(cl, bleMac, "TRANSIT_SUCCESS", 100, totalFiles, totalFiles)
                scheduleResetIdle(cl, bleMac)
            } catch (_: InterruptedException) {
                LocalDaemonClient.cancelOShare(batchId)
                updateDeviceProgress(cl, bleMac, "CANCEL", 0, succeeded, totalFiles)
                scheduleResetIdle(cl, bleMac)
            } catch (err: Throwable) {
                WorkspaceLog.error("oshare_send", "LAN OShare transfer failed", err)
                updateDeviceProgress(cl, bleMac, "TRANSIT_FAILED", 0, succeeded, totalFiles)
                val daemonRecorded = streamAttempted && snapshot.transfers.any { it.id == batchId }
                if (!daemonRecorded) {
                    FluidCloudBridge.showTransferError(
                        ctx,
                        "send",
                        peerName,
                        items.firstOrNull()?.fileName ?: "OPPO 分享",
                        err.message ?: "请检查连接",
                    )
                }
                scheduleResetIdle(cl, bleMac)
            } finally {
                activeCancelled.remove(bleMac)
            }
        }
    }

    fun cancelLanSend(targetDevice: Any) {
        val cl = appClassLoader ?: return
        val bleMac = invokeNoArg(targetDevice, "a") as? String ?: return
        val peerId = WorkspaceContract.parseDeviceIdFromOshareMac(bleMac) ?: return
        activeCancelled[bleMac]?.set(true)
        sendExecutor.execute {
            LocalDaemonClient.cancelOShare(peerId)
        }
        updateDeviceProgress(cl, bleMac, "CANCEL", 0, 0, 1)
        scheduleResetIdle(cl, bleMac)
    }

    private fun updateDeviceProgress(
        cl: ClassLoader,
        bleMac: String,
        stateName: String,
        progress: Int,
        succeedNum: Int,
        totalNum: Int,
    ) {
        Handler(Looper.getMainLooper()).post {
            runCatching {
                val dmClass = cl.loadClass("v9.a")
                val dm = dmClass.getMethod("h").invoke(null) ?: return@post
                val dev = dmClass.getMethod("m", String::class.java).invoke(dm, bleMac) ?: return@post
                invokeOneArg(dev, "H", progress)
                invokeOneArg(dev, "J", succeedNum)
                invokeOneArg(dev, "K", totalNum)
                enumConstant(cl, "com.oplus.oshare.OplusOshareState", stateName)?.let { st ->
                    invokeOneArg(dev, "I", st)
                }
                dmClass.getMethod("r").invoke(dm)
            }
        }
    }

    private fun scheduleResetIdle(cl: ClassLoader, bleMac: String) {
        Handler(Looper.getMainLooper()).postDelayed({
            updateDeviceProgress(cl, bleMac, "IDLE", 0, 0, 1)
        }, 2500L)
    }

    private sealed interface SendPayloadItem {
        val fileName: String
        val size: Long
        fun openStream(cr: ContentResolver): java.io.InputStream
    }

    private data class UriSendItem(
        val uri: Uri,
        override val fileName: String,
        override val size: Long,
    ) : SendPayloadItem {
        override fun openStream(cr: ContentResolver): java.io.InputStream =
            if (uri.scheme == ContentResolver.SCHEME_FILE && uri.path != null) {
                FileInputStream(File(uri.path!!))
            } else {
                cr.openInputStream(uri) ?: error("无法读取分享文件: $fileName")
            }
    }

    private data class TextSendItem(
        val bytes: ByteArray,
        override val fileName: String,
    ) : SendPayloadItem {
        override val size: Long = bytes.size.toLong()
        override fun openStream(cr: ContentResolver): java.io.InputStream = ByteArrayInputStream(bytes)
    }

    @Suppress("DEPRECATION")
    private fun extractSendItems(ctx: Context, intent: Intent, cl: ClassLoader): List<SendPayloadItem> {
        val uris = LinkedHashSet<Uri>()
        var recordTextFallback: String? = null

        // 1. ColorOS SharedRecord parcelables (Gallery, Files app, & ShareHandlerActivity)
        runCatching {
            val extras = intent.extras
            if (extras != null) {
                val srCls = runCatching { cl.loadClass("com.coloros.oshare.model.SharedRecord") }
                    .getOrElse { runCatching { cl.loadClass("com.oplus.oshare.task.SharedRecord") }.getOrNull() }
                if (srCls != null) {
                    extras.classLoader = srCls.classLoader
                }
                val list = extras.get("sharedRecords") as? List<*>
                list?.forEach { rec ->
                    val uri = extractUriFromRecord(rec)
                    if (uri != null) {
                        uris.add(uri)
                    } else if (recordTextFallback.isNullOrEmpty()) {
                        recordTextFallback = extractTextFromRecord(rec)
                    }
                }
            }
        }

        // 2. OShare DEVICE_LIST Data records
        runCatching {
            val dataCls = runCatching { cl.loadClass("com.oplus.oshare.task.Data") }.getOrNull()
            if (dataCls != null) {
                intent.extras?.classLoader = dataCls.classLoader
            }
            val dataList = intent.getSerializableExtra("data_list") as? List<*>
            dataList?.forEach { item ->
                val uri = invokeNoArg(item, "s") as? Uri
                    ?: (invokeNoArg(item, "m") as? String)?.takeIf { it.isNotBlank() }?.let { Uri.fromFile(File(it)) }
                if (uri != null) uris.add(uri)
            }
        }

        // 3. Standard ACTION_SEND / ACTION_SEND_MULTIPLE
        if (Intent.ACTION_SEND_MULTIPLE == intent.action) {
            val list = if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
            }
            list?.let { uris.addAll(it) }
        } else {
            val single: Uri? = if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableExtra<Parcelable>(Intent.EXTRA_STREAM) as? Uri
            }
            single?.let { uris.add(it) }
        }
        intent.data?.let { uris.add(it) }

        // 4. ClipData fallback
        val clip: ClipData? = intent.clipData
        if (clip != null) {
            for (i in 0 until clip.itemCount) {
                clip.getItemAt(i).uri?.let { uris.add(it) }
            }
        }

        if (uris.isNotEmpty()) {
            val cr = ctx.contentResolver
            return uris.map { uri -> resolveUriMetadata(cr, uri) }
        }

        // 5. Plain shared text fallback
        val sharedText = intent.getStringExtra(Intent.EXTRA_TEXT)
            ?: recordTextFallback
            ?: clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()
        if (!sharedText.isNullOrEmpty()) {
            return listOf(
                TextSendItem(
                    bytes = sharedText.toByteArray(StandardCharsets.UTF_8),
                    fileName = "shared_text_${System.currentTimeMillis()}.txt",
                ),
            )
        }
        return emptyList()
    }

    private fun extractUriFromRecord(record: Any?): Uri? {
        if (record == null) return null
        if (record is Uri) return record
        if (record is String) return parseNonEmptyUriString(record)
        for (methodName in listOf("getUri", "c", "b", "a")) {
            val v = runCatching { record.javaClass.getMethod(methodName).invoke(record) }.getOrNull()
            if (v is Uri) return v
            if (v is String) {
                parseNonEmptyUriString(v)?.let { return it }
            }
        }
        for (field in record.javaClass.declaredFields) {
            if (Uri::class.java.isAssignableFrom(field.type)) {
                field.isAccessible = true
                (field.get(record) as? Uri)?.let { return it }
            } else if (field.type == String::class.java && field.name.contains("uri", ignoreCase = true)) {
                field.isAccessible = true
                parseNonEmptyUriString(field.get(record) as? String)?.let { return it }
            }
        }
        return null
    }

    private fun parseNonEmptyUriString(raw: String?): Uri? {
        val trimmed = raw?.trim()?.takeIf { it.isNotEmpty() } ?: return null
        return runCatching { Uri.parse(trimmed) }.getOrNull()
    }

    private fun extractTextFromRecord(record: Any?): String? {
        if (record == null) return null
        for (methodName in listOf("getContent", "getText")) {
            val v = runCatching { record.javaClass.getMethod(methodName).invoke(record) as? String }.getOrNull()
            if (!v.isNullOrEmpty()) return v
        }
        return null
    }

    private fun resolveUriMetadata(cr: ContentResolver, uri: Uri): UriSendItem {
        if (uri.scheme == ContentResolver.SCHEME_FILE && uri.path != null) {
            val f = File(uri.path!!)
            return UriSendItem(uri, f.name.ifEmpty { "file" }, f.length())
        }

        var name: String? = null
        var size: Long = -1L
        runCatching {
            cr.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)
                ?.use { cursor ->
                    if (cursor.moveToFirst()) {
                        val nameIdx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                        if (nameIdx >= 0) name = cursor.getString(nameIdx)
                        val sizeIdx = cursor.getColumnIndex(OpenableColumns.SIZE)
                        if (sizeIdx >= 0 && !cursor.isNull(sizeIdx)) size = cursor.getLong(sizeIdx)
                    }
                }
        }
        if (size < 0L) {
            runCatching {
                cr.openAssetFileDescriptor(uri, "r")?.use { afd ->
                    if (afd.length >= 0L) size = afd.length
                }
            }
        }
        val cleanName = name?.takeIf { it.isNotBlank() }
            ?: uri.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() }
            ?: "shared_${System.currentTimeMillis()}"
        return UriSendItem(uri, cleanName, size)
    }

    private fun enumConstant(cl: ClassLoader, clsName: String, constName: String): Any? =
        runCatching {
            val cls = cl.loadClass(clsName)
            cls.enumConstants?.firstOrNull { (it as Enum<*>).name == constName }
        }.getOrNull()

    private fun invokeNoArg(target: Any?, methodName: String): Any? {
        if (target == null) return null
        return runCatching {
            target.javaClass.getMethod(methodName).invoke(target)
        }.getOrNull()
    }

    private fun invokeOneArg(target: Any, methodName: String, arg: Any?) {
        runCatching {
            val m = target.javaClass.methods.firstOrNull { cand ->
                cand.name == methodName && cand.parameterCount == 1 && isArgCompatible(cand.parameterTypes[0], arg)
            } ?: target.javaClass.getDeclaredMethods().firstOrNull { cand ->
                cand.name == methodName && cand.parameterCount == 1 && isArgCompatible(cand.parameterTypes[0], arg)
            } ?: return
            m.isAccessible = true
            m.invoke(target, arg)
        }
    }

    private fun isArgCompatible(param: Class<*>, arg: Any?): Boolean {
        if (arg == null) return !param.isPrimitive
        if (param.isAssignableFrom(arg.javaClass)) return true
        return when (param) {
            Int::class.javaPrimitiveType -> arg is Int
            Boolean::class.javaPrimitiveType -> arg is Boolean
            Long::class.javaPrimitiveType -> arg is Long
            else -> false
        }
    }
}
