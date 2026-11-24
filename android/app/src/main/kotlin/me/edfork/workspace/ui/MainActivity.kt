package me.edfork.workspace.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandHorizontally
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.shrinkHorizontally
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat
import java.util.Calendar
import java.util.concurrent.Executors
import me.edfork.workspace.BuildConfig
import me.edfork.workspace.R
import me.edfork.workspace.config.XposedServiceBridge
import me.edfork.workspace.ipc.LocalDaemonClient
import me.edfork.workspace.liveupdate.FluidCloudBridge
import me.edfork.workspace.liveupdate.FluidCloudController
import me.edfork.workspace.model.PairedPeerStatus
import me.edfork.workspace.model.WorkspaceContract
import me.edfork.workspace.model.WorkspaceSnapshot

private enum class GlassPage(val title: String, val shortLabel: String, val icon: GlassIconKind) {
    DEVICES("设备状态", "设备", GlassIconKind.DEVICES),
    CLIPBOARD("剪贴板共享", "剪贴板", GlassIconKind.CLIPBOARD),
    TRANSFERS("文件传输", "传输", GlassIconKind.TRANSFER),
    SCREEN("屏幕共享", "投屏", GlassIconKind.SCREEN),
    NOTES("OPPO 笔记", "笔记", GlassIconKind.NOTES),
}

private enum class GlassModal {
    NONE,
    SETTINGS,
    DISCOVER,
}

class MainActivity : ComponentActivity() {
    private val worker = Executors.newSingleThreadExecutor { r ->
        Thread(r, "workspace-ui-io").apply { isDaemon = true }
    }
    private var notificationsEnabled by mutableStateOf(false)
    private var promotedEnabled by mutableStateOf(false)
    private var customFrostedBitmap by mutableStateOf<ImageBitmap?>(null)
    private var customThumbBitmap by mutableStateOf<ImageBitmap?>(null)
    private var activePreset by mutableStateOf(GlassWallpaperPreset.AURORA)

    private val notificationPermissionLauncher = registerForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) {
        refreshFluidCloudPermissionState()
    }

    private val pickWallpaperLauncher = registerForActivityResult(
        ActivityResultContracts.GetContent(),
    ) { uri ->
        if (uri == null) return@registerForActivityResult
        runIo {
            WallpaperRepository.importAndBlurUri(this@MainActivity, uri)
                .onSuccess { (frosted, thumb) ->
                    runOnUiThread {
                        customFrostedBitmap = frosted
                        customThumbBitmap = thumb
                        activePreset = GlassWallpaperPreset.CUSTOM
                        getSharedPreferences("workspace_ui_prefs", MODE_PRIVATE)
                            .edit()
                            .putString("wallpaper_preset", GlassWallpaperPreset.CUSTOM.prefValue)
                            .apply()
                    }
                }
                .onFailure { err ->
                    showToast("导入壁纸失败: ${err.message}")
                }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)

        FluidCloudController.ensureChannel(this)
        refreshFluidCloudPermissionState()

        val prefs = getSharedPreferences("workspace_ui_prefs", MODE_PRIVATE)
        val initialTheme = GlassThemeMode.fromPref(prefs.getString("theme_mode", null))
        activePreset = GlassWallpaperPreset.fromPref(prefs.getString("wallpaper_preset", null))

        // Load cached pre-blurred 540px wallpaper asynchronously so startup frame 1 never blocks on disk I/O
        runIo {
            val (frosted, thumb) = WallpaperRepository.loadCachedBitmaps(this@MainActivity)
            if (frosted != null || thumb != null) {
                runOnUiThread {
                    customFrostedBitmap = frosted
                    customThumbBitmap = thumb
                }
            }
        }

        setContent {
            var themeMode by remember { mutableStateOf(initialTheme) }
            WorkspaceGlassTheme(themeMode = themeMode) {
                val palette = LocalGlassPalette.current
                SideEffect {
                    val controller = WindowCompat.getInsetsController(window, window.decorView)
                    controller.isAppearanceLightStatusBars = !palette.isDark
                    controller.isAppearanceLightNavigationBars = !palette.isDark
                }

                var snap by remember { mutableStateOf(LocalDaemonClient.latestSnapshot) }
                val frameworkState by XposedServiceBridge.stateFlow.collectAsState()

                DisposableEffect(Unit) {
                    val snapListener: (WorkspaceSnapshot) -> Unit = { updated ->
                        snap = updated
                        syncSystemDeviceNameIfNeeded(updated)
                    }
                    LocalDaemonClient.addListener(snapListener)
                    LocalDaemonClient.startSseMonitorIfNeeded()
                    worker.execute { LocalDaemonClient.fetchStatusSync() }
                    onDispose {
                        LocalDaemonClient.removeListener(snapListener)
                    }
                }

                WorkspaceFrostedScreen(
                    snapshot = snap,
                    frameworkState = frameworkState,
                    notificationsEnabled = notificationsEnabled,
                    promotedEnabled = promotedEnabled,
                    themeMode = themeMode,
                    onThemeModeChange = { next ->
                        themeMode = next
                        prefs.edit().putString("theme_mode", next.prefValue).apply()
                    },
                    wallpaperPreset = activePreset,
                    customFrosted = customFrostedBitmap,
                    customThumb = customThumbBitmap,
                    onWallpaperPresetChange = { next ->
                        if (next == GlassWallpaperPreset.CUSTOM && customFrostedBitmap == null) {
                            pickWallpaperLauncher.launch("image/*")
                        } else {
                            activePreset = next
                            prefs.edit().putString("wallpaper_preset", next.prefValue).apply()
                        }
                    },
                    onPickCustomWallpaper = {
                        pickWallpaperLauncher.launch("image/*")
                    },
                    onRequestFluidCloudPermission = { requestFluidCloudPermission() },
                    onRefresh = {
                        refreshFluidCloudPermissionState()
                        XposedServiceBridge.refreshStatus()
                        runIo { LocalDaemonClient.fetchStatusSync() }
                    },
                    onStartPair = { id ->
                        runIo {
                            val r = LocalDaemonClient.startPair(id)
                            if (r.isFailure) showToast("发起配对失败: ${r.exceptionOrNull()?.message}")
                        }
                    },
                    onApprovePair = { sid ->
                        runIo { LocalDaemonClient.approvePair(sid) }
                    },
                    onRejectPair = { sid ->
                        runIo { LocalDaemonClient.rejectPair(sid) }
                    },
                    onUnpair = { id ->
                        runIo { LocalDaemonClient.unpair(id) }
                    },
                    onPushClip = { peerId ->
                        val peerName = snap.pairedPeers.firstOrNull { it.deviceId == peerId }?.deviceName
                        FluidCloudBridge.showClipboardProgress(this@MainActivity, "push", peerName)
                        runIo {
                            val r = LocalDaemonClient.triggerClipboardAction("push", peerId)
                            if (r.ok) {
                                FluidCloudBridge.showClipboardDone(
                                    this@MainActivity,
                                    "push",
                                    r.peerName.ifBlank { peerName },
                                    r.message,
                                )
                            } else {
                                FluidCloudBridge.showClipboardError(this@MainActivity, "push", r.message)
                            }
                        }
                    },
                    onPullClip = { peerId ->
                        val peerName = snap.pairedPeers.firstOrNull { it.deviceId == peerId }?.deviceName
                        FluidCloudBridge.showClipboardProgress(this@MainActivity, "pull", peerName)
                        runIo {
                            val r = LocalDaemonClient.triggerClipboardAction("pull", peerId)
                            if (r.ok) {
                                FluidCloudBridge.showClipboardDone(
                                    this@MainActivity,
                                    "pull",
                                    r.peerName.ifBlank { peerName },
                                    r.message,
                                )
                            } else {
                                FluidCloudBridge.showClipboardError(this@MainActivity, "pull", r.message)
                            }
                        }
                    },
                    onToggleGesture = { enabled ->
                        runIo { LocalDaemonClient.updateConfig(gestureEnabled = enabled) }
                    },
                    onToggleSyncPaused = { paused ->
                        runIo { LocalDaemonClient.updateConfig(syncPaused = paused) }
                    },
                    onSaveUserName = { name ->
                        runIo {
                            val ok = LocalDaemonClient.updateConfig(userName = name.trim()).isSuccess
                            showToast(if (ok) "用户名已保存" else "保存用户名失败")
                        }
                    },
                    onSaveDeviceName = { name ->
                        runIo {
                            val trimmed = name.trim()
                            val ok = LocalDaemonClient.updateConfig(deviceName = trimmed).isSuccess
                            if (ok && trimmed.isEmpty()) {
                                readSystemDeviceName()?.let { sysName ->
                                    LocalDaemonClient.updateConfig(systemDeviceName = sysName)
                                }
                            }
                            showToast(if (ok) "本机名称已保存" else "保存本机名称失败")
                        }
                    },
                )
            }
        }
    }

    override fun onResume() {
        super.onResume()
        refreshFluidCloudPermissionState()
        XposedServiceBridge.refreshStatus()
        runIo {
            LocalDaemonClient.fetchStatusSync().onSuccess { snap ->
                syncSystemDeviceNameIfNeeded(snap)
            }
        }
    }

    private fun readSystemDeviceName(): String? {
        val candidates = listOfNotNull(
            runCatching { Settings.Global.getString(contentResolver, Settings.Global.DEVICE_NAME) }.getOrNull(),
            runCatching { Settings.Secure.getString(contentResolver, "bluetooth_name") }.getOrNull(),
        )
        return candidates
            .map { it.replace("\r", "").replace("\n", " ").trim() }
            .firstOrNull { it.isNotEmpty() && !it.equals("null", ignoreCase = true) }
            ?.take(64)
    }

    private fun syncSystemDeviceNameIfNeeded(snap: WorkspaceSnapshot) {
        if (!snap.daemonReachable || snap.deviceNameCustomized) return
        val sysName = readSystemDeviceName() ?: return
        if (sysName != snap.deviceName) {
            runIo { LocalDaemonClient.updateConfig(systemDeviceName = sysName) }
        }
    }

    private fun refreshFluidCloudPermissionState() {
        notificationsEnabled = FluidCloudController.areNotificationsEnabled(this)
        promotedEnabled = FluidCloudController.canPostPromoted(this)
    }

    private fun requestFluidCloudPermission() {
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
            return
        }
        val intents = buildList {
            if (Build.VERSION.SDK_INT >= 36 && notificationsEnabled && !promotedEnabled) {
                add(
                    Intent(Settings.ACTION_APP_NOTIFICATION_PROMOTION_SETTINGS).apply {
                        putExtra(Settings.EXTRA_APP_PACKAGE, packageName)
                        putExtra("app_uid", applicationInfo.uid)
                    },
                )
            }
            add(
                Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).apply {
                    putExtra(Settings.EXTRA_APP_PACKAGE, packageName)
                },
            )
            add(
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                    data = Uri.fromParts("package", packageName, null)
                },
            )
        }
        for (intent in intents) {
            val started = runCatching { startActivity(intent) }.isSuccess
            if (started) return
        }
    }

    private fun runIo(block: () -> Unit) {
        worker.execute(block)
    }

    private fun showToast(msg: String) {
        runOnUiThread {
            Toast.makeText(this, msg, Toast.LENGTH_SHORT).show()
        }
    }
}

private fun timeGreetingPrefix(): String {
    val hour = Calendar.getInstance().get(Calendar.HOUR_OF_DAY)
    return when {
        hour < 6 -> "凌晨好"
        hour < 9 -> "早上好"
        hour < 12 -> "上午好"
        hour < 14 -> "中午好"
        hour < 18 -> "下午好"
        else -> "晚上好"
    }
}

@Composable
private fun WorkspaceFrostedScreen(
    snapshot: WorkspaceSnapshot,
    frameworkState: XposedServiceBridge.FrameworkState,
    notificationsEnabled: Boolean,
    promotedEnabled: Boolean,
    themeMode: GlassThemeMode,
    onThemeModeChange: (GlassThemeMode) -> Unit,
    wallpaperPreset: GlassWallpaperPreset,
    customFrosted: ImageBitmap?,
    customThumb: ImageBitmap?,
    onWallpaperPresetChange: (GlassWallpaperPreset) -> Unit,
    onPickCustomWallpaper: () -> Unit,
    onRequestFluidCloudPermission: () -> Unit,
    onRefresh: () -> Unit,
    onStartPair: (String) -> Unit,
    onApprovePair: (String) -> Unit,
    onRejectPair: (String) -> Unit,
    onUnpair: (String) -> Unit,
    onPushClip: (String) -> Unit,
    onPullClip: (String) -> Unit,
    onToggleGesture: (Boolean) -> Unit,
    onToggleSyncPaused: (Boolean) -> Unit,
    onSaveUserName: (String) -> Unit,
    onSaveDeviceName: (String) -> Unit,
) {
    val g = LocalGlassPalette.current
    var activePage by remember { mutableStateOf(GlassPage.DEVICES) }
    var activeModal by remember { mutableStateOf(GlassModal.NONE) }
    var lastModalContent by remember { mutableStateOf(GlassModal.SETTINGS) }
    var refreshSpinTicks by remember { mutableIntStateOf(0) }

    val refreshSpinAngle by animateFloatAsState(
        targetValue = refreshSpinTicks * 360f,
        animationSpec = tween(durationMillis = 540, easing = FastOutSlowInEasing),
        label = "refreshSpin",
    )
    val triggerRefresh = {
        refreshSpinTicks += 1
        onRefresh()
    }

    if (activeModal != GlassModal.NONE) {
        lastModalContent = activeModal
    }

    val stageBlurRadius by animateDpAsState(
        targetValue = if (activeModal != GlassModal.NONE) 18.dp else 0.dp,
        animationSpec = tween(durationMillis = 240, easing = FastOutSlowInEasing),
        label = "stageBlur",
    )

    BackHandler(enabled = activeModal != GlassModal.NONE) {
        activeModal = GlassModal.NONE
    }

    BoxWithConstraints(modifier = Modifier.fillMaxSize()) {
        val wideLayout = maxWidth >= 600.dp
        val stageBlurModifier = if (stageBlurRadius > 0.5.dp) Modifier.blur(stageBlurRadius) else Modifier

        // Both pre-blurred wallpaper and stage live inside blurred layer when modal opens
        Box(
            modifier = Modifier
                .fillMaxSize()
                .then(stageBlurModifier),
        ) {
            FrostedWallpaperLayer(
                preset = wallpaperPreset,
                customFrosted = customFrosted,
                palette = g,
                modifier = Modifier.fillMaxSize(),
            )

            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .windowInsetsPadding(WindowInsets.safeDrawing),
            ) {
                if (wideLayout) {
                    Row(modifier = Modifier.fillMaxSize()) {
                        FrostedSidebar(
                            activePage = activePage,
                            onSelectPage = { activePage = it },
                            onOpenSettings = { activeModal = GlassModal.SETTINGS },
                        )
                        Box(modifier = Modifier.weight(1f).fillMaxHeight()) {
                            StageContent(
                                activePage = activePage,
                                snapshot = snapshot,
                                frameworkState = frameworkState,
                                notificationsEnabled = notificationsEnabled,
                                promotedEnabled = promotedEnabled,
                                onRequestFluidCloudPermission = onRequestFluidCloudPermission,
                                onRefresh = triggerRefresh,
                                refreshSpinAngle = refreshSpinAngle,
                                onOpenDiscover = { activeModal = GlassModal.DISCOVER },
                                onApprovePair = onApprovePair,
                                onRejectPair = onRejectPair,
                                onPushClip = onPushClip,
                                onPullClip = onPullClip,
                                onToggleGesture = onToggleGesture,
                                onToggleSyncPaused = onToggleSyncPaused,
                                wideStage = true,
                            )
                            Box(
                                modifier = Modifier
                                    .align(Alignment.BottomEnd)
                                    .padding(end = 26.dp, bottom = 24.dp),
                            ) {
                                androidx.compose.animation.AnimatedVisibility(
                                    visible = activePage == GlassPage.DEVICES,
                                    enter = scaleIn(
                                        initialScale = 0.72f,
                                        animationSpec = spring(dampingRatio = 0.70f, stiffness = 500f),
                                    ) + fadeIn(tween(180)),
                                    exit = scaleOut(targetScale = 0.78f, animationSpec = tween(140)) + fadeOut(tween(140)),
                                ) {
                                    FrostedFab(onClick = { activeModal = GlassModal.DISCOVER })
                                }
                            }
                        }
                    }
                } else {
                    Box(modifier = Modifier.fillMaxSize()) {
                        Column(modifier = Modifier.fillMaxSize()) {
                            FrostedCompactTopBar(
                                snapshot = snapshot,
                                refreshSpinAngle = refreshSpinAngle,
                                onRefresh = triggerRefresh,
                                onOpenSettings = { activeModal = GlassModal.SETTINGS },
                            )
                            Box(modifier = Modifier.weight(1f).fillMaxWidth()) {
                                StageContent(
                                    activePage = activePage,
                                    snapshot = snapshot,
                                    frameworkState = frameworkState,
                                    notificationsEnabled = notificationsEnabled,
                                    promotedEnabled = promotedEnabled,
                                    onRequestFluidCloudPermission = onRequestFluidCloudPermission,
                                    onRefresh = triggerRefresh,
                                    refreshSpinAngle = refreshSpinAngle,
                                    onOpenDiscover = { activeModal = GlassModal.DISCOVER },
                                    onApprovePair = onApprovePair,
                                    onRejectPair = onRejectPair,
                                    onPushClip = onPushClip,
                                    onPullClip = onPullClip,
                                    onToggleGesture = onToggleGesture,
                                    onToggleSyncPaused = onToggleSyncPaused,
                                    wideStage = false,
                                    extraBottomPaddingDp = 88,
                                )
                                Box(
                                    modifier = Modifier
                                        .align(Alignment.BottomEnd)
                                        .padding(end = 20.dp, bottom = 92.dp),
                                ) {
                                    androidx.compose.animation.AnimatedVisibility(
                                        visible = activePage == GlassPage.DEVICES,
                                        enter = scaleIn(
                                            initialScale = 0.72f,
                                            animationSpec = spring(dampingRatio = 0.70f, stiffness = 500f),
                                        ) + fadeIn(tween(180)),
                                        exit = scaleOut(targetScale = 0.78f, animationSpec = tween(140)) + fadeOut(tween(140)),
                                    ) {
                                        FrostedFab(onClick = { activeModal = GlassModal.DISCOVER })
                                    }
                                }
                            }
                        }
                        FrostedBottomDock(
                            activePage = activePage,
                            onSelectPage = { activePage = it },
                            modifier = Modifier
                                .align(Alignment.BottomCenter)
                                .padding(horizontal = 14.dp, vertical = 10.dp),
                        )
                    }
                }
            }
        }

        AnimatedVisibility(
            visible = activeModal != GlassModal.NONE,
            enter = fadeIn(animationSpec = tween(200)),
            exit = fadeOut(animationSpec = tween(170)),
        ) {
            FrostedModalOverlay(
                visible = activeModal != GlassModal.NONE,
                modalKind = if (activeModal != GlassModal.NONE) activeModal else lastModalContent,
                snapshot = snapshot,
                frameworkState = frameworkState,
                notificationsEnabled = notificationsEnabled,
                promotedEnabled = promotedEnabled,
                themeMode = themeMode,
                onThemeModeChange = onThemeModeChange,
                wallpaperPreset = wallpaperPreset,
                customThumb = customThumb,
                onWallpaperPresetChange = onWallpaperPresetChange,
                onPickCustomWallpaper = onPickCustomWallpaper,
                onRequestFluidCloudPermission = onRequestFluidCloudPermission,
                refreshSpinAngle = refreshSpinAngle,
                onRefresh = triggerRefresh,
                onStartPair = { id ->
                    activeModal = GlassModal.NONE
                    onStartPair(id)
                },
                onUnpair = onUnpair,
                onToggleGesture = onToggleGesture,
                onToggleSyncPaused = onToggleSyncPaused,
                onSaveUserName = onSaveUserName,
                onSaveDeviceName = onSaveDeviceName,
                onDismiss = { activeModal = GlassModal.NONE },
            )
        }
    }
}

@Composable
private fun BrandLogoAvatar(sizeDp: Int = 42) {
    Image(
        painter = painterResource(R.drawable.brand_logo),
        contentDescription = "寰宇工作台",
        contentScale = ContentScale.Crop,
        modifier = Modifier
            .size(sizeDp.dp)
            .clip(CircleShape)
            .border(1.dp, Color.White.copy(alpha = 0.55f), CircleShape),
    )
}

@Composable
private fun FrostedSidebar(
    activePage: GlassPage,
    onSelectPage: (GlassPage) -> Unit,
    onOpenSettings: () -> Unit,
) {
    val g = LocalGlassPalette.current
    Column(
        modifier = Modifier
            .width(236.dp)
            .fillMaxHeight()
            .background(Brush.verticalGradient(listOf(g.sidebarTop, g.sidebarBottom)))
            .border(1.dp, g.sidebarBorder)
            .padding(start = 14.dp, end = 14.dp, top = 20.dp, bottom = 16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(bottom = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            BrandLogoAvatar(42)
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                GlassText("寰宇工作台", fontSizeSp = 16, weight = FontWeight.ExtraBold)
                GlassText(
                    "${BuildConfig.VERSION_DISPLAY} · 海纳百川，有容乃大",
                    color = g.textSecondary,
                    fontSizeSp = 10,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }

        GlassPage.entries.forEach { page ->
            GlassNavCapsule(
                label = page.title,
                icon = page.icon,
                active = activePage == page,
                onClick = { onSelectPage(page) },
            )
        }

        Spacer(modifier = Modifier.weight(1f))

        GlassNavCapsule(
            label = "设置",
            icon = GlassIconKind.SETTINGS,
            active = false,
            onClick = onOpenSettings,
        )
    }
}

@Composable
private fun FrostedCompactTopBar(
    snapshot: WorkspaceSnapshot,
    refreshSpinAngle: Float,
    onRefresh: () -> Unit,
    onOpenSettings: () -> Unit,
) {
    val g = LocalGlassPalette.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(Brush.verticalGradient(listOf(g.sidebarTop, g.sidebarBottom)))
            .border(1.dp, g.sidebarBorder)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        BrandLogoAvatar(36)
        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
            Row(
                verticalAlignment = Alignment.Bottom,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                GlassText("寰宇工作台", fontSizeSp = 15, weight = FontWeight.ExtraBold, maxLines = 1)
                GlassText(
                    BuildConfig.VERSION_DISPLAY,
                    color = g.textSecondary,
                    fontSizeSp = 11,
                    weight = FontWeight.SemiBold,
                    maxLines = 1,
                )
            }
            GlassText(
                text = when {
                    snapshot.lanConnected -> "局域网已连接 · TLS :${snapshot.tlsPort}"
                    snapshot.daemonReachable -> "守护在线 · 等待电脑"
                    else -> "等待 workspaced 守护..."
                },
                color = g.textSecondary,
                fontSizeSp = 11,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        CircularGlassIconButton(
            icon = GlassIconKind.REFRESH,
            iconRotationDeg = refreshSpinAngle,
            onClick = onRefresh,
        )
        CircularGlassIconButton(
            icon = GlassIconKind.SETTINGS,
            onClick = onOpenSettings,
        )
    }
}

@Composable
private fun FrostedBottomDock(
    activePage: GlassPage,
    onSelectPage: (GlassPage) -> Unit,
    modifier: Modifier = Modifier,
) {
    val g = LocalGlassPalette.current
    Row(
        modifier = modifier
            .fillMaxWidth()
            .shadow(16.dp, CircleShape, ambientColor = Color.Black.copy(alpha = 0.16f))
            .clip(CircleShape)
            .background(Brush.verticalGradient(listOf(g.modalTop, g.modalBottom)))
            .border(1.dp, g.cardBorder, CircleShape)
            .padding(horizontal = 8.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.SpaceEvenly,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        GlassPage.entries.forEach { page ->
            val active = activePage == page
            val interaction = remember { MutableInteractionSource() }
            val pressed by interaction.collectIsPressedAsState()
            val scale by animateFloatAsState(
                targetValue = if (pressed) 0.94f else 1f,
                animationSpec = spring(dampingRatio = 0.72f, stiffness = 620f),
                label = "dockItemScale",
            )
            val pillBgTop by animateColorAsState(
                targetValue = if (active) g.navActiveTop else Color.Transparent,
                animationSpec = tween(180),
                label = "dockTop",
            )
            val pillBgBot by animateColorAsState(
                targetValue = if (active) g.navActiveBottom else Color.Transparent,
                animationSpec = tween(180),
                label = "dockBot",
            )
            val pillBorder by animateColorAsState(
                targetValue = if (active) g.navActiveBorder else Color.Transparent,
                animationSpec = tween(180),
                label = "dockBorder",
            )
            val padH by animateDpAsState(
                targetValue = if (active) 12.dp else 10.dp,
                animationSpec = spring(stiffness = 500f),
                label = "dockPad",
            )

            Row(
                modifier = Modifier
                    .graphicsLayer {
                        scaleX = scale
                        scaleY = scale
                    }
                    .clip(CircleShape)
                    .background(Brush.verticalGradient(listOf(pillBgTop, pillBgBot)))
                    .border(1.dp, pillBorder, CircleShape)
                    .clickable(interactionSource = interaction, indication = null) { onSelectPage(page) }
                    .padding(horizontal = padH, vertical = 7.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                if (active) {
                    GlassNavBadgeIcon(kind = page.icon, size = 24.dp)
                } else {
                    GlassIcon(kind = page.icon, tint = g.textSecondary, size = 18.dp)
                }
                AnimatedVisibility(
                    visible = active,
                    enter = expandHorizontally(tween(200, easing = FastOutSlowInEasing)) + fadeIn(tween(180)),
                    exit = shrinkHorizontally(tween(150)) + fadeOut(tween(120)),
                ) {
                    GlassText(
                        text = page.shortLabel,
                        fontSizeSp = 12,
                        weight = FontWeight.Bold,
                        maxLines = 1,
                        softWrap = false,
                    )
                }
            }
        }
    }
}

@Composable
private fun CircularGlassIconButton(
    icon: GlassIconKind,
    onClick: () -> Unit,
    sizeDp: Int = 36,
    iconRotationDeg: Float = 0f,
) {
    val g = LocalGlassPalette.current
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(
        targetValue = if (pressed) 0.92f else 1f,
        animationSpec = spring(dampingRatio = 0.72f, stiffness = 620f),
        label = "circBtnScale",
    )
    Box(
        modifier = Modifier
            .size(sizeDp.dp)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .clip(CircleShape)
            .background(Brush.verticalGradient(listOf(g.secBtnTop, g.secBtnBottom)))
            .border(1.dp, g.secBtnBorder, CircleShape)
            .clickable(interactionSource = interaction, indication = null, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        GlassIcon(
            kind = icon,
            tint = g.secBtnFg,
            size = 16.dp,
            modifier = if (iconRotationDeg != 0f) {
                Modifier.graphicsLayer { rotationZ = iconRotationDeg }
            } else {
                Modifier
            },
        )
    }
}

@Composable
private fun FrostedFab(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val g = LocalGlassPalette.current
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(
        targetValue = if (pressed) 0.92f else 1f,
        animationSpec = spring(dampingRatio = 0.70f, stiffness = 600f),
        label = "fabScale",
    )
    Box(
        modifier = modifier
            .size(56.dp)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .shadow(14.dp, CircleShape, ambientColor = Color.Black.copy(alpha = 0.22f))
            .clip(CircleShape)
            .background(Brush.verticalGradient(listOf(g.fabTop, g.fabBottom)))
            .border(1.dp, g.fabBorder, CircleShape)
            .clickable(interactionSource = interaction, indication = null, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        GlassIcon(kind = GlassIconKind.PLUS, tint = g.fabFg, size = 22.dp)
    }
}

@Composable
private fun StageContent(
    activePage: GlassPage,
    snapshot: WorkspaceSnapshot,
    frameworkState: XposedServiceBridge.FrameworkState,
    notificationsEnabled: Boolean,
    promotedEnabled: Boolean,
    onRequestFluidCloudPermission: () -> Unit,
    onRefresh: () -> Unit,
    refreshSpinAngle: Float,
    onOpenDiscover: () -> Unit,
    onApprovePair: (String) -> Unit,
    onRejectPair: (String) -> Unit,
    onPushClip: (String) -> Unit,
    onPullClip: (String) -> Unit,
    onToggleGesture: (Boolean) -> Unit,
    onToggleSyncPaused: (Boolean) -> Unit,
    wideStage: Boolean,
    extraBottomPaddingDp: Int = 24,
) {
    val padH = if (wideStage) 28.dp else 16.dp
    val padTop = if (wideStage) 24.dp else 14.dp

    AnimatedContent(
        targetState = activePage,
        transitionSpec = {
            val forward = targetState.ordinal >= initialState.ordinal
            (
                fadeIn(animationSpec = tween(220, easing = FastOutSlowInEasing)) +
                    slideInVertically(animationSpec = tween(260, easing = FastOutSlowInEasing)) { fullH ->
                        if (forward) fullH / 16 else -fullH / 16
                    }
                ) togetherWith fadeOut(animationSpec = tween(150))
        },
        label = "stagePageTransition",
    ) { page ->
        when (page) {
            GlassPage.DEVICES -> DevicesPage(
                snapshot = snapshot,
                frameworkState = frameworkState,
                notificationsEnabled = notificationsEnabled,
                promotedEnabled = promotedEnabled,
                onRequestFluidCloudPermission = onRequestFluidCloudPermission,
                onRefresh = onRefresh,
                refreshSpinAngle = refreshSpinAngle,
                onOpenDiscover = onOpenDiscover,
                onApprovePair = onApprovePair,
                onRejectPair = onRejectPair,
                onPushClip = onPushClip,
                onPullClip = onPullClip,
                wideStage = wideStage,
                padH = padH,
                padTop = padTop,
                padBottom = extraBottomPaddingDp.dp,
            )
            GlassPage.CLIPBOARD -> ClipboardPage(
                snapshot = snapshot,
                onToggleSyncPaused = onToggleSyncPaused,
                onToggleGesture = onToggleGesture,
                onPushClip = onPushClip,
                onPullClip = onPullClip,
                padH = padH,
                padTop = padTop,
                padBottom = extraBottomPaddingDp.dp,
            )
            GlassPage.TRANSFERS -> TransfersPage(
                snapshot = snapshot,
                padH = padH,
                padTop = padTop,
                padBottom = extraBottomPaddingDp.dp,
            )
            GlassPage.SCREEN -> ConstructionPage(
                heading = "屏幕共享",
                centerTitle = "SCrcpy 集成",
                description = "无线 ADB 低延迟镜像与跨屏键鼠协同通道接入中",
                padH = padH,
                padTop = padTop,
                padBottom = extraBottomPaddingDp.dp,
            )
            GlassPage.NOTES -> ConstructionPage(
                heading = "OPPO 笔记",
                centerTitle = "OPPO 笔记同步",
                description = "跨端笔记实时双向同步与富文本解析模块开发中",
                padH = padH,
                padTop = padTop,
                padBottom = extraBottomPaddingDp.dp,
            )
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun DevicesPage(
    snapshot: WorkspaceSnapshot,
    frameworkState: XposedServiceBridge.FrameworkState,
    notificationsEnabled: Boolean,
    promotedEnabled: Boolean,
    onRequestFluidCloudPermission: () -> Unit,
    onRefresh: () -> Unit,
    refreshSpinAngle: Float,
    onOpenDiscover: () -> Unit,
    onApprovePair: (String) -> Unit,
    onRejectPair: (String) -> Unit,
    onPushClip: (String) -> Unit,
    onPullClip: (String) -> Unit,
    wideStage: Boolean,
    padH: Dp,
    padTop: Dp,
    padBottom: Dp,
) {
    val g = LocalGlassPalette.current
    val scroll = rememberScrollState()
    val xposedConn = frameworkState as? XposedServiceBridge.FrameworkState.Connected
    val fluidReady = notificationsEnabled && (Build.VERSION.SDK_INT < 36 || promotedEnabled)
    val onlineCount = snapshot.onlinePairedPeers.size

    BoxWithConstraints(modifier = Modifier.fillMaxSize()) {
        val twoColumns = maxWidth >= 640.dp
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(scroll)
                .padding(start = padH, end = padH, top = padTop, bottom = padBottom),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val userDisplay = snapshot.userName.ifBlank { "User" }
                GlassText(
                    text = "${timeGreetingPrefix()}，$userDisplay !",
                    fontSizeSp = if (wideStage) 28 else 24,
                    weight = FontWeight.ExtraBold,
                    letterSpacing = (-0.5).sp,
                    modifier = Modifier.weight(1f),
                )
                if (wideStage) {
                    Spacer(modifier = Modifier.width(10.dp))
                    GlassButton(
                        label = "刷新状态",
                        icon = GlassIconKind.REFRESH,
                        iconRotationDeg = refreshSpinAngle,
                        onClick = onRefresh,
                    )
                }
            }

            GlassCard(padding = PaddingValues(horizontal = 16.dp, vertical = 12.dp), spacing = 8.dp) {
                FlowRow(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    GlassStatusPill(
                        label = if (snapshot.daemonReachable) "KSU 守护运行中" else "KSU 未启动",
                        tone = if (snapshot.daemonReachable) GlassPillTone.ONLINE else GlassPillTone.OFFLINE,
                        dot = true,
                    )
                    GlassStatusPill(
                        label = if (xposedConn != null) "${xposedConn.frameworkName} API ${xposedConn.apiVersion}" else "LSPosed 未连接",
                        tone = if (xposedConn != null) GlassPillTone.ONLINE else GlassPillTone.AMBER,
                    )
                    GlassStatusPill(
                        label = when {
                            !notificationsEnabled -> "缺通知权限"
                            Build.VERSION.SDK_INT >= 36 && !promotedEnabled -> "流体云未授权"
                            else -> "流体云已就绪"
                        },
                        tone = if (fluidReady) GlassPillTone.ONLINE else GlassPillTone.AMBER,
                    )
                }
                if (xposedConn != null || !fluidReady) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        val scopeText = if (xposedConn != null) {
                            val sysOk = xposedConn.scope.contains(WorkspaceContract.SYSTEM_UI_PACKAGE)
                            val oshareOk = xposedConn.scope.contains(WorkspaceContract.OSHARE_PACKAGE)
                            "作用域：SystemUI=${if (sysOk) "已勾选" else "未勾选"} · OPPO分享=${if (oshareOk) "已勾选" else "未勾选"}"
                        } else {
                            "开启通知与实况更新权后，传输/剪贴板进度将实时上流体云"
                        }
                        GlassText(
                            text = scopeText,
                            color = g.textSecondary,
                            fontSizeSp = 11,
                            modifier = Modifier.weight(1f),
                        )
                        if (!fluidReady) {
                            Spacer(modifier = Modifier.width(8.dp))
                            GlassButton(
                                label = "去开启",
                                variant = GlassButtonVariant.PRIMARY,
                                onClick = onRequestFluidCloudPermission,
                                padding = PaddingValues(horizontal = 12.dp, vertical = 5.dp),
                            )
                        }
                    }
                }
            }

            snapshot.pendingPairs.forEach { sas ->
                GlassCard(
                    topColor = g.sasTop,
                    bottomColor = g.sasBottom,
                    borderColor = g.sasBorder,
                    cornerRadius = 20.dp,
                ) {
                    GlassText(
                        text = if (sas.isInitiator) "待电脑确认配对：${sas.peerName}" else "收到配对请求：${sas.peerName}",
                        fontSizeSp = 16,
                        weight = FontWeight.Bold,
                    )
                    Box(modifier = Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                        GlassText(
                            text = sas.sasCode,
                            color = g.sasCodeFg,
                            fontSizeSp = 30,
                            weight = FontWeight.ExtraBold,
                            mono = true,
                            letterSpacing = 5.sp,
                        )
                    }
                    GlassText(
                        text = "请核对电脑端 6 位验证码一致 · 指纹 ${sas.peerFingerprint}",
                        color = g.textMono,
                        fontSizeSp = 11,
                        mono = true,
                    )
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.End,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        GlassButton(
                            label = "拒绝",
                            variant = GlassButtonVariant.DANGER,
                            onClick = { onRejectPair(sas.sessionId) },
                        )
                        Spacer(modifier = Modifier.width(8.dp))
                        if (sas.state != "approved_local") {
                            GlassButton(
                                label = "确认配对",
                                variant = GlassButtonVariant.PRIMARY,
                                onClick = { onApprovePair(sas.sessionId) },
                            )
                        } else {
                            GlassStatusPill(label = "等待电脑确认", tone = GlassPillTone.AMBER)
                        }
                    }
                }
            }

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                GlassText("已连接的设备", fontSizeSp = 14, weight = FontWeight.Bold)
                GlassStatusPill(
                    label = if (onlineCount > 0) "$onlineCount 台已连接 · TLS :${snapshot.tlsPort}" else "等待设备 · TLS :${snapshot.tlsPort}",
                    tone = GlassPillTone.INFO,
                )
            }

            if (snapshot.pairedPeers.isEmpty()) {
                GlassCard {
                    GlassText("暂无已配对电脑", fontSizeSp = 17, weight = FontWeight.Bold)
                    GlassText(
                        "点击右下角 + 按钮发现同一局域网下的电脑并发起 6 位验证码配对",
                        color = g.textSecondary,
                        fontSizeSp = 13,
                    )
                    GlassButton(
                        label = "发现局域网电脑",
                        variant = GlassButtonVariant.PRIMARY,
                        onClick = onOpenDiscover,
                    )
                }
            } else if (twoColumns) {
                snapshot.pairedPeers.chunked(2).forEach { rowPeers ->
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(16.dp),
                    ) {
                        rowPeers.forEach { peer ->
                            DeviceFrostedCard(
                                peer = peer,
                                onPush = { onPushClip(peer.deviceId) },
                                onPull = { onPullClip(peer.deviceId) },
                                modifier = Modifier.weight(1f),
                            )
                        }
                        if (rowPeers.size == 1) {
                            Spacer(modifier = Modifier.weight(1f))
                        }
                    }
                }
            } else {
                snapshot.pairedPeers.forEach { peer ->
                    DeviceFrostedCard(
                        peer = peer,
                        onPush = { onPushClip(peer.deviceId) },
                        onPull = { onPullClip(peer.deviceId) },
                    )
                }
            }
        }
    }
}

@Composable
private fun DeviceFrostedCard(
    peer: PairedPeerStatus,
    onPush: () -> Unit,
    onPull: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val g = LocalGlassPalette.current
    GlassCard(modifier = modifier, spacing = 10.dp) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            GlassText(
                text = peer.deviceName,
                fontSizeSp = 17,
                weight = FontWeight.Bold,
                maxLines = 1,
                softWrap = false,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Spacer(modifier = Modifier.width(8.dp))
            GlassStatusPill(
                label = if (peer.connected) "已连接" else "离线",
                tone = if (peer.connected) GlassPillTone.ONLINE else GlassPillTone.OFFLINE,
            )
        }

        val meta = if (peer.connected && !peer.ip.isNullOrBlank()) {
            "${peer.ip}:${peer.tlsPort} · ${peer.fingerprint.take(8)}"
        } else {
            "指纹 ${peer.fingerprint.take(12)}"
        }
        GlassText(
            text = meta,
            color = g.textMono,
            fontSizeSp = 11,
            mono = true,
            maxLines = 1,
            softWrap = false,
            overflow = TextOverflow.Ellipsis,
        )

        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(top = 6.dp),
            verticalAlignment = Alignment.Bottom,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                GlassButton(
                    label = "推送",
                    icon = GlassIconKind.PUSH,
                    enabled = peer.connected,
                    onClick = onPush,
                    padding = PaddingValues(horizontal = 12.dp, vertical = 6.dp),
                )
                GlassButton(
                    label = "拉取",
                    icon = GlassIconKind.PULL,
                    enabled = peer.connected,
                    onClick = onPull,
                    padding = PaddingValues(horizontal = 12.dp, vertical = 6.dp),
                )
            }

            Spacer(modifier = Modifier.width(12.dp))

            val rtt = peer.rttMs
            val (heroTxt, capTxt, heroColor) = when {
                !peer.connected -> Triple("离线", "等待设备上线", g.metricMuted)
                rtt != null -> Triple(
                    "${rtt}ms",
                    "局域网心跳延迟",
                    when {
                        rtt < 25L -> g.metricFast
                        rtt < 90L -> g.metricWarm
                        else -> g.metricSlow
                    },
                )
                else -> Triple("已连", "mTLS 通道就绪", g.metricFast)
            }
            Column(horizontalAlignment = Alignment.End) {
                GlassText(
                    text = heroTxt,
                    color = heroColor,
                    fontSizeSp = 38,
                    weight = FontWeight.ExtraBold,
                    letterSpacing = (-1.2).sp,
                    maxLines = 1,
                    softWrap = false,
                )
                GlassText(
                    text = capTxt,
                    color = g.textSecondary,
                    fontSizeSp = 11,
                    weight = FontWeight.SemiBold,
                    maxLines = 1,
                    softWrap = false,
                )
            }
        }
    }
}

@Composable
private fun ClipboardPage(
    snapshot: WorkspaceSnapshot,
    onToggleSyncPaused: (Boolean) -> Unit,
    onToggleGesture: (Boolean) -> Unit,
    onPushClip: (String) -> Unit,
    onPullClip: (String) -> Unit,
    padH: Dp,
    padTop: Dp,
    padBottom: Dp,
) {
    val g = LocalGlassPalette.current
    val scroll = rememberScrollState()
    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(scroll)
            .padding(start = padH, end = padH, top = padTop, bottom = padBottom),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        GlassText("剪贴板共享", fontSizeSp = 26, weight = FontWeight.ExtraBold)
        GlassText(
            "Android-master 无损双向剪贴板同步 · 传输与进度通过流体云实时显示",
            color = g.textSecondary,
            fontSizeSp = 13,
        )

        GlassCard {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                    GlassText("启用剪贴板同步", fontSizeSp = 16, weight = FontWeight.Bold)
                    GlassText("暂停期间仍保持 mTLS 心跳连接，文件直传不受影响", color = g.textSecondary, fontSizeSp = 12)
                }
                GlassStatusPill(
                    label = if (snapshot.syncPaused) "已暂停" else "同步中",
                    tone = if (snapshot.syncPaused) GlassPillTone.AMBER else GlassPillTone.ONLINE,
                )
                GlassSwitch(
                    checked = !snapshot.syncPaused,
                    onCheckedChange = { enabled -> onToggleSyncPaused(!enabled) },
                )
            }
        }

        GlassCard {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                    GlassText("双指手势快捷传剪贴板", fontSizeSp = 16, weight = FontWeight.Bold)
                    GlassText(
                        if (snapshot.isGestureActive) {
                            "已激活：双指上滑=${snapshot.swipeUpAction} · 下滑=${snapshot.swipeDownAction}"
                        } else {
                            "仅连接已配对电脑且开启时生效，离线零触摸拦截"
                        },
                        color = if (snapshot.isGestureActive) g.pillOnlineFg else g.textSecondary,
                        fontSizeSp = 12,
                        weight = FontWeight.Medium,
                    )
                }
                GlassSwitch(
                    checked = snapshot.gestureEnabled,
                    onCheckedChange = onToggleGesture,
                )
            }
        }

        if (snapshot.onlinePairedPeers.isNotEmpty()) {
            GlassCard {
                GlassText("向在线电脑手动触发", fontSizeSp = 14, weight = FontWeight.Bold)
                snapshot.onlinePairedPeers.forEach { peer ->
                    GlassSubRow {
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            GlassText(
                                text = peer.deviceName,
                                fontSizeSp = 14,
                                weight = FontWeight.SemiBold,
                                maxLines = 1,
                                softWrap = false,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.weight(1f),
                            )
                            GlassStatusPill(label = "已连接", tone = GlassPillTone.ONLINE, dot = true)
                        }
                        Spacer(modifier = Modifier.height(2.dp))
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            GlassButton(
                                label = "推送剪贴板",
                                variant = GlassButtonVariant.PRIMARY,
                                icon = GlassIconKind.PUSH,
                                onClick = { onPushClip(peer.deviceId) },
                                modifier = Modifier.weight(1f),
                                padding = PaddingValues(horizontal = 12.dp, vertical = 9.dp),
                            )
                            GlassButton(
                                label = "拉取剪贴板",
                                icon = GlassIconKind.PULL,
                                onClick = { onPullClip(peer.deviceId) },
                                modifier = Modifier.weight(1f),
                                padding = PaddingValues(horizontal = 12.dp, vertical = 9.dp),
                            )
                        }
                    }
                }
            }
        }

        GlassCard {
            GlassText("最近一次同步记录", fontSizeSp = 14, weight = FontWeight.Bold)
            val lc = snapshot.lastClipboard
            if (lc == null) {
                GlassText(
                    "暂无同步记录。在手机上使用双指手势或按钮触发，或在电脑端复制文本/图片。",
                    color = g.textSecondary,
                    fontSizeSp = 13,
                )
            } else {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    val dirLabel = if (lc.direction == "send") "手机 -> 电脑" else "电脑 -> 手机"
                    val kindLabel = if (lc.kind == "image") "图片" else "文本"
                    GlassStatusPill(label = dirLabel, tone = GlassPillTone.INFO)
                    GlassText(
                        "对端 ${lc.peerName} · $kindLabel · ${formatHumanBytes(lc.size.toLong())}",
                        fontSizeSp = 13,
                        weight = FontWeight.SemiBold,
                    )
                }
            }
        }
    }
}

@Composable
private fun TransfersPage(
    snapshot: WorkspaceSnapshot,
    padH: Dp,
    padTop: Dp,
    padBottom: Dp,
) {
    val g = LocalGlassPalette.current
    val scroll = rememberScrollState()
    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(scroll)
            .padding(start = padH, end = padH, top = padTop, bottom = padBottom),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        GlassText("文件传输", fontSizeSp = 26, weight = FontWeight.ExtraBold)
        GlassText(
            "mTLS + BLAKE3 流式直传，系统分享选中 OPPO 互传里的电脑即可发送，进度实时上流体云",
            color = g.textSecondary,
            fontSizeSp = 13,
        )

        GlassCard {
            GlassText("文件接收目录", fontSizeSp = 14, weight = FontWeight.Bold)
            GlassText(snapshot.receiveDir, color = g.textMono, fontSizeSp = 12, mono = true)
        }

        if (snapshot.transfers.isEmpty()) {
            GlassCard {
                GlassText("暂无文件传输记录", fontSizeSp = 15, weight = FontWeight.Bold)
                GlassText(
                    "在任意应用中点击系统「分享」，选择 OPPO 互传列表中的已配对电脑即可高速传输。",
                    color = g.textSecondary,
                    fontSizeSp = 13,
                )
            }
        } else {
            snapshot.transfers.take(15).forEach { tr ->
                GlassCard(spacing = 8.dp) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        GlassStatusPill(
                            label = if (tr.direction == "send") "发送" else "接收",
                            tone = GlassPillTone.INFO,
                        )
                        GlassText(
                            text = tr.summary,
                            fontSizeSp = 14,
                            weight = FontWeight.Bold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                        val (stLabel, stTone) = when (tr.state) {
                            "transferring" -> "传输中" to GlassPillTone.AMBER
                            "completed" -> "已完成" to GlassPillTone.ONLINE
                            "failed" -> "失败" to GlassPillTone.DANGER
                            else -> "已取消" to GlassPillTone.OFFLINE
                        }
                        GlassStatusPill(label = stLabel, tone = stTone)
                    }
                    GlassText(
                        text = "${tr.peerName} · ${formatHumanBytes(tr.bytesTransferred)} / ${formatHumanBytes(tr.totalBytes)}",
                        color = g.textMono,
                        fontSizeSp = 11,
                        mono = true,
                    )
                    val frac = when {
                        tr.totalBytes > 0L -> (tr.bytesTransferred.toFloat() / tr.totalBytes.toFloat()).coerceIn(0f, 1f)
                        tr.state == "completed" -> 1f
                        else -> 0f
                    }
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(7.dp)
                            .clip(CircleShape)
                            .background(g.switchOffBg),
                    ) {
                        Box(
                            modifier = Modifier
                                .fillMaxWidth(frac)
                                .fillMaxHeight()
                                .clip(CircleShape)
                                .background(Brush.horizontalGradient(listOf(Color(0xFF3B82F6), Color(0xFF0EA5E9)))),
                        )
                    }
                    tr.errorMessage?.takeIf { it.isNotBlank() }?.let { err ->
                        GlassStatusPill(label = err, tone = GlassPillTone.DANGER)
                    }
                }
            }
        }
    }
}

@Composable
private fun ConstructionPage(
    heading: String,
    centerTitle: String,
    description: String,
    padH: Dp,
    padTop: Dp,
    padBottom: Dp,
) {
    val g = LocalGlassPalette.current
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(start = padH, end = padH, top = padTop, bottom = padBottom),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        GlassText(heading, fontSizeSp = 26, weight = FontWeight.ExtraBold)
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .weight(1f)
                .clip(RoundedCornerShape(22.dp))
                .background(Brush.linearGradient(listOf(g.canvasTop, g.canvasBottom)))
                .border(1.dp, g.cardBorder, RoundedCornerShape(22.dp))
                .padding(24.dp),
            contentAlignment = Alignment.Center,
        ) {
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                GlassText(centerTitle, fontSizeSp = 22, weight = FontWeight.ExtraBold)
                GlassStatusPill(label = "施工中", tone = GlassPillTone.AMBER)
                GlassText(description, color = g.textSecondary, fontSizeSp = 13)
            }
        }
    }
}

@Composable
private fun FrostedModalOverlay(
    visible: Boolean,
    modalKind: GlassModal,
    snapshot: WorkspaceSnapshot,
    frameworkState: XposedServiceBridge.FrameworkState,
    notificationsEnabled: Boolean,
    promotedEnabled: Boolean,
    themeMode: GlassThemeMode,
    onThemeModeChange: (GlassThemeMode) -> Unit,
    wallpaperPreset: GlassWallpaperPreset,
    customThumb: ImageBitmap?,
    onWallpaperPresetChange: (GlassWallpaperPreset) -> Unit,
    onPickCustomWallpaper: () -> Unit,
    onRequestFluidCloudPermission: () -> Unit,
    refreshSpinAngle: Float,
    onRefresh: () -> Unit,
    onStartPair: (String) -> Unit,
    onUnpair: (String) -> Unit,
    onToggleGesture: (Boolean) -> Unit,
    onToggleSyncPaused: (Boolean) -> Unit,
    onSaveUserName: (String) -> Unit,
    onSaveDeviceName: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val g = LocalGlassPalette.current
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(g.modalScrim)
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                onClick = onDismiss,
            )
            .windowInsetsPadding(WindowInsets.safeDrawing)
            .padding(horizontal = 20.dp, vertical = 24.dp),
        contentAlignment = Alignment.Center,
    ) {
        val modalShape = RoundedCornerShape(26.dp)
        AnimatedVisibility(
            visible = visible,
            enter = scaleIn(
                initialScale = 0.92f,
                animationSpec = spring(dampingRatio = 0.78f, stiffness = 480f),
            ) + fadeIn(tween(200)),
            exit = scaleOut(targetScale = 0.94f, animationSpec = tween(160)) + fadeOut(tween(160)),
        ) {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .widthIn(max = 520.dp)
                    .shadow(24.dp, modalShape, ambientColor = Color.Black.copy(alpha = 0.25f))
                    .clip(modalShape)
                    .background(Brush.linearGradient(listOf(g.modalTop, g.modalBottom)))
                    .border(1.dp, g.modalBorder, modalShape)
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        onClick = {},
                    )
                    .padding(22.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    GlassText(
                        text = if (modalKind == GlassModal.SETTINGS) "设置" else "同一局域网发现电脑",
                        fontSizeSp = 20,
                        weight = FontWeight.ExtraBold,
                    )
                    Box(
                        modifier = Modifier
                            .size(40.dp)
                            .shadow(6.dp, CircleShape, ambientColor = Color.Black.copy(alpha = 0.14f))
                            .clip(CircleShape)
                            .background(Color.White)
                            .border(1.dp, Color(0xFF0F172A).copy(alpha = 0.08f), CircleShape)
                            .clickable(onClick = onDismiss),
                        contentAlignment = Alignment.Center,
                    ) {
                        GlassIcon(kind = GlassIconKind.CLOSE, tint = Color(0xFF111319), size = 15.dp)
                    }
                }

                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    if (modalKind == GlassModal.SETTINGS) {
                        SettingsModalContent(
                            snapshot = snapshot,
                            frameworkState = frameworkState,
                            notificationsEnabled = notificationsEnabled,
                            promotedEnabled = promotedEnabled,
                            themeMode = themeMode,
                            onThemeModeChange = onThemeModeChange,
                            wallpaperPreset = wallpaperPreset,
                            customThumb = customThumb,
                            onWallpaperPresetChange = onWallpaperPresetChange,
                            onPickCustomWallpaper = onPickCustomWallpaper,
                            onRequestFluidCloudPermission = onRequestFluidCloudPermission,
                            onUnpair = onUnpair,
                            onToggleGesture = onToggleGesture,
                            onToggleSyncPaused = onToggleSyncPaused,
                            onSaveUserName = onSaveUserName,
                            onSaveDeviceName = onSaveDeviceName,
                        )
                    } else {
                        DiscoverModalContent(
                            snapshot = snapshot,
                            refreshSpinAngle = refreshSpinAngle,
                            onRefresh = onRefresh,
                            onStartPair = onStartPair,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun SettingsModalContent(
    snapshot: WorkspaceSnapshot,
    frameworkState: XposedServiceBridge.FrameworkState,
    notificationsEnabled: Boolean,
    promotedEnabled: Boolean,
    themeMode: GlassThemeMode,
    onThemeModeChange: (GlassThemeMode) -> Unit,
    wallpaperPreset: GlassWallpaperPreset,
    customThumb: ImageBitmap?,
    onWallpaperPresetChange: (GlassWallpaperPreset) -> Unit,
    onPickCustomWallpaper: () -> Unit,
    onRequestFluidCloudPermission: () -> Unit,
    onUnpair: (String) -> Unit,
    onToggleGesture: (Boolean) -> Unit,
    onToggleSyncPaused: (Boolean) -> Unit,
    onSaveUserName: (String) -> Unit,
    onSaveDeviceName: (String) -> Unit,
) {
    val g = LocalGlassPalette.current
    val focusManager = LocalFocusManager.current
    val fluidReady = notificationsEnabled && (Build.VERSION.SDK_INT < 36 || promotedEnabled)
    val xposedConn = frameworkState as? XposedServiceBridge.FrameworkState.Connected

    var userNameInput by remember(snapshot.userName) { mutableStateOf(snapshot.userName) }
    var deviceNameInput by remember(snapshot.deviceName) { mutableStateOf(snapshot.deviceName) }

    GlassSubRow {
        GlassText("用户名", fontSizeSp = 13, weight = FontWeight.Bold)
        GlassText("默认: User，显示于首页问候语，留空保存恢复默认", color = g.textSecondary, fontSizeSp = 11)
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            GlassTextField(
                value = userNameInput,
                onValueChange = { userNameInput = it },
                placeholder = "User",
                modifier = Modifier.weight(1f),
                onDone = {
                    focusManager.clearFocus()
                    onSaveUserName(userNameInput)
                },
            )
            GlassButton(
                label = "保存",
                variant = GlassButtonVariant.PRIMARY,
                onClick = {
                    focusManager.clearFocus()
                    onSaveUserName(userNameInput)
                },
                padding = PaddingValues(horizontal = 14.dp, vertical = 9.dp),
            )
        }
    }

    GlassSubRow {
        GlassText("本机名称", fontSizeSp = 13, weight = FontWeight.Bold)
        GlassText("默认跟随系统本机名称，在对端电脑显示，留空保存恢复系统名称", color = g.textSecondary, fontSizeSp = 11)
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            GlassTextField(
                value = deviceNameInput,
                onValueChange = { deviceNameInput = it },
                placeholder = Build.MODEL ?: "OPPO Android",
                modifier = Modifier.weight(1f),
                onDone = {
                    focusManager.clearFocus()
                    onSaveDeviceName(deviceNameInput)
                },
            )
            GlassButton(
                label = "保存",
                variant = GlassButtonVariant.PRIMARY,
                onClick = {
                    focusManager.clearFocus()
                    onSaveDeviceName(deviceNameInput)
                },
                padding = PaddingValues(horizontal = 14.dp, vertical = 9.dp),
            )
        }
    }

    GlassSubRow {
        GlassText("玻璃主题外观", fontSizeSp = 13, weight = FontWeight.Bold)
        Spacer(modifier = Modifier.height(2.dp))
        BoxWithConstraints(
            modifier = Modifier
                .fillMaxWidth()
                .height(36.dp)
                .clip(CircleShape)
                .background(g.segWrapBg)
                .padding(3.dp),
        ) {
            val modes = GlassThemeMode.entries
            val segWidth = maxWidth / modes.size
            val selectedIndex = modes.indexOf(themeMode).coerceAtLeast(0)
            val thumbOffset by animateDpAsState(
                targetValue = segWidth * selectedIndex,
                animationSpec = spring(dampingRatio = 0.78f, stiffness = 520f),
                label = "themeThumbOffset",
            )
            Box(
                modifier = Modifier
                    .offset(x = thumbOffset)
                    .width(segWidth)
                    .fillMaxHeight()
                    .clip(CircleShape)
                    .background(g.segActiveBg),
            )
            Row(modifier = Modifier.fillMaxSize()) {
                modes.forEach { mode ->
                    val active = themeMode == mode
                    Box(
                        modifier = Modifier
                            .weight(1f)
                            .fillMaxHeight()
                            .clip(CircleShape)
                            .clickable(
                                interactionSource = remember { MutableInteractionSource() },
                                indication = null,
                            ) { onThemeModeChange(mode) },
                        contentAlignment = Alignment.Center,
                    ) {
                        GlassText(
                            text = mode.label,
                            color = if (active) g.segActiveFg else g.textSecondary,
                            fontSizeSp = 12,
                            weight = FontWeight.Bold,
                            maxLines = 1,
                            softWrap = false,
                        )
                    }
                }
            }
        }
    }

    // Background Wallpaper Picker (presets + pre-blurred custom photo, zero runtime blur overhead)
    GlassSubRow {
        GlassText("背景壁纸（本地磨砂渲染）", fontSizeSp = 13, weight = FontWeight.Bold)
        Spacer(modifier = Modifier.height(2.dp))
        BoxWithConstraints(
            modifier = Modifier
                .fillMaxWidth()
                .height(36.dp)
                .clip(CircleShape)
                .background(g.segWrapBg)
                .padding(3.dp),
        ) {
            val presets = GlassWallpaperPreset.entries
            val segWidth = maxWidth / presets.size
            val selectedIndex = presets.indexOf(wallpaperPreset).coerceAtLeast(0)
            val thumbOffset by animateDpAsState(
                targetValue = segWidth * selectedIndex,
                animationSpec = spring(dampingRatio = 0.78f, stiffness = 520f),
                label = "wpThumbOffset",
            )
            Box(
                modifier = Modifier
                    .offset(x = thumbOffset)
                    .width(segWidth)
                    .fillMaxHeight()
                    .clip(CircleShape)
                    .background(g.segActiveBg),
            )
            Row(modifier = Modifier.fillMaxSize()) {
                presets.forEach { p ->
                    val active = wallpaperPreset == p
                    Box(
                        modifier = Modifier
                            .weight(1f)
                            .fillMaxHeight()
                            .clip(CircleShape)
                            .clickable(
                                interactionSource = remember { MutableInteractionSource() },
                                indication = null,
                            ) { onWallpaperPresetChange(p) },
                        contentAlignment = Alignment.Center,
                    ) {
                        GlassText(
                            text = p.label,
                            color = if (active) g.segActiveFg else g.textSecondary,
                            fontSizeSp = 12,
                            weight = FontWeight.Bold,
                            maxLines = 1,
                            softWrap = false,
                        )
                    }
                }
            }
        }
        Spacer(modifier = Modifier.height(4.dp))
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (customThumb != null) {
                Image(
                    bitmap = customThumb,
                    contentDescription = "当前自定义壁纸预览",
                    contentScale = ContentScale.Crop,
                    modifier = Modifier
                        .size(40.dp)
                        .clip(RoundedCornerShape(10.dp))
                        .border(1.dp, g.cardBorder, RoundedCornerShape(10.dp)),
                )
            }
            GlassButton(
                label = if (customThumb != null) "更换相册壁纸" else "从相册选择壁纸",
                icon = GlassIconKind.IMAGE,
                onClick = onPickCustomWallpaper,
                modifier = Modifier.weight(1f),
                padding = PaddingValues(horizontal = 14.dp, vertical = 9.dp),
            )
        }
    }

    GlassSubRow {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Column(modifier = Modifier.weight(1f)) {
                GlassText("ColorOS 流体云实时更新", fontSizeSp = 13, weight = FontWeight.Bold)
                GlassText(
                    text = if (fluidReady) "通知与实况胶囊权限已就绪" else "待授予通知及实时更新权限",
                    color = g.textSecondary,
                    fontSizeSp = 11,
                )
            }
            if (fluidReady) {
                GlassStatusPill(label = "已就绪", tone = GlassPillTone.ONLINE)
            } else {
                GlassButton(
                    label = "开启权限",
                    variant = GlassButtonVariant.PRIMARY,
                    onClick = onRequestFluidCloudPermission,
                )
            }
        }
    }

    GlassSubRow {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            GlassText("启用跨端剪贴板实时同步", fontSizeSp = 13, weight = FontWeight.Bold)
            GlassSwitch(
                checked = !snapshot.syncPaused,
                onCheckedChange = { enabled -> onToggleSyncPaused(!enabled) },
            )
        }
    }
    GlassSubRow {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            GlassText("局域网门控双指剪贴板手势", fontSizeSp = 13, weight = FontWeight.Bold)
            GlassSwitch(
                checked = snapshot.gestureEnabled,
                onCheckedChange = onToggleGesture,
            )
        }
    }

    GlassText("已配对电脑管理", fontSizeSp = 13, weight = FontWeight.Bold)
    if (snapshot.pairedPeers.isEmpty()) {
        GlassText("暂无已配对电脑", color = g.textSecondary, fontSizeSp = 12)
    } else {
        snapshot.pairedPeers.forEach { peer ->
            GlassSubRow {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Column(modifier = Modifier.weight(1f)) {
                        GlassText(peer.deviceName, fontSizeSp = 13, weight = FontWeight.Bold)
                        GlassText(
                            "${peer.ip ?: "离线"}:${peer.tlsPort} · ${peer.fingerprint.take(12)}",
                            color = g.textMono,
                            fontSizeSp = 11,
                            mono = true,
                        )
                    }
                    GlassButton(
                        label = "取消配对",
                        variant = GlassButtonVariant.DANGER,
                        onClick = { onUnpair(peer.deviceId) },
                    )
                }
            }
        }
    }

    GlassText("本机节点与模块信息", fontSizeSp = 13, weight = FontWeight.Bold)
    GlassSubRow {
        GlassText(
            "软件版本: ${BuildConfig.VERSION_DISPLAY} (${BuildConfig.VERSION_NAME})",
            color = g.textMono,
            fontSizeSp = 11,
            mono = true,
        )
        GlassText(
            "设备名称: ${snapshot.deviceName.ifBlank { "--" }}",
            color = g.textMono,
            fontSizeSp = 11,
            mono = true,
        )
        GlassText(
            "TLS 指纹: ${snapshot.fingerprint.ifBlank { "--" }}",
            color = g.textMono,
            fontSizeSp = 11,
            mono = true,
        )
        GlassText("监听端口: ${snapshot.tlsPort}", color = g.textMono, fontSizeSp = 11, mono = true)
        GlassText("接收目录: ${snapshot.receiveDir}", color = g.textMono, fontSizeSp = 11, mono = true)
        val xpText = if (xposedConn != null) {
            "${xposedConn.frameworkName} (API ${xposedConn.apiVersion}) · SystemUI=${xposedConn.scope.contains(WorkspaceContract.SYSTEM_UI_PACKAGE)} · OShare=${xposedConn.scope.contains(WorkspaceContract.OSHARE_PACKAGE)}"
        } else {
            "Xposed 服务未连接"
        }
        GlassText("模块状态: $xpText", color = g.textMono, fontSizeSp = 11, mono = true)
    }
}

@Composable
private fun DiscoverModalContent(
    snapshot: WorkspaceSnapshot,
    refreshSpinAngle: Float,
    onRefresh: () -> Unit,
    onStartPair: (String) -> Unit,
) {
    val g = LocalGlassPalette.current
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        GlassText(
            "确保 Linux 端「寰宇工作台」处于同一 Wi-Fi",
            color = g.textSecondary,
            fontSizeSp = 12,
            modifier = Modifier.weight(1f),
        )
        GlassButton(
            label = "刷新",
            icon = GlassIconKind.REFRESH,
            iconRotationDeg = refreshSpinAngle,
            onClick = onRefresh,
            padding = PaddingValues(horizontal = 12.dp, vertical = 6.dp),
        )
    }

    if (snapshot.discoveredPeers.isEmpty()) {
        GlassSubRow {
            GlassText(
                "正在扫描同一局域网下的 me-edfork-workspace 信标...",
                color = g.textSecondary,
                fontSizeSp = 12,
            )
        }
    } else {
        snapshot.discoveredPeers.forEach { d ->
            GlassSubRow {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        GlassText(d.deviceName, fontSizeSp = 14, weight = FontWeight.Bold)
                        GlassText(
                            "${d.ip}:${d.tlsPort} · ${d.fingerprint.take(10)}",
                            color = g.textMono,
                            fontSizeSp = 11,
                            mono = true,
                        )
                    }
                    GlassButton(
                        label = if (d.isTrusted) "已配对" else "发起 6 位码配对",
                        variant = if (d.isTrusted) GlassButtonVariant.SECONDARY else GlassButtonVariant.PRIMARY,
                        enabled = !d.isTrusted,
                        onClick = { onStartPair(d.deviceId) },
                    )
                }
            }
        }
    }
}
