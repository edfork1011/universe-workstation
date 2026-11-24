package me.edfork.workspace.ui

import androidx.compose.animation.Crossfade
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import me.edfork.workspace.R

enum class GlassThemeMode(val prefValue: String, val label: String) {
    LIGHT("light", "浅色"),
    DARK("dark", "深色"),
    SYSTEM("system", "跟随系统");

    companion object {
        fun fromPref(raw: String?): GlassThemeMode =
            entries.firstOrNull { it.prefValue == raw } ?: LIGHT
    }
}

enum class GlassIconKind {
    DEVICES,
    CLIPBOARD,
    TRANSFER,
    SCREEN,
    NOTES,
    SETTINGS,
    PLUS,
    CLOSE,
    REFRESH,
    PUSH,
    PULL,
    IMAGE,
}

enum class GlassButtonVariant {
    PRIMARY,
    SECONDARY,
    DANGER,
}

enum class GlassPillTone {
    ONLINE,
    OFFLINE,
    AMBER,
    INFO,
    DANGER,
}

@Immutable
data class GlassPalette(
    val isDark: Boolean,
    val shellTop: Color,
    val shellBottom: Color,
    val shellBorder: Color,
    val sidebarTop: Color,
    val sidebarBottom: Color,
    val sidebarBorder: Color,
    val navIdleTop: Color,
    val navIdleBottom: Color,
    val navIdleBorder: Color,
    val navActiveTop: Color,
    val navActiveBottom: Color,
    val navActiveBorder: Color,
    val cardTop: Color,
    val cardBottom: Color,
    val cardBorder: Color,
    val specularLine: Color,
    val subRowBg: Color,
    val subRowBorder: Color,
    val canvasTop: Color,
    val canvasBottom: Color,
    val modalTop: Color,
    val modalBottom: Color,
    val modalBorder: Color,
    val modalScrim: Color,
    val segWrapBg: Color,
    val segActiveBg: Color,
    val segActiveFg: Color,
    val textPrimary: Color,
    val textSecondary: Color,
    val textMono: Color,
    val metricFast: Color,
    val metricWarm: Color,
    val metricSlow: Color,
    val metricMuted: Color,
    val pillOnlineBg: Color,
    val pillOnlineBorder: Color,
    val pillOnlineFg: Color,
    val pillOfflineBg: Color,
    val pillOfflineBorder: Color,
    val pillOfflineFg: Color,
    val pillAmberBg: Color,
    val pillAmberBorder: Color,
    val pillAmberFg: Color,
    val pillInfoBg: Color,
    val pillInfoBorder: Color,
    val pillInfoFg: Color,
    val pillDangerBg: Color,
    val pillDangerBorder: Color,
    val pillDangerFg: Color,
    val secBtnTop: Color,
    val secBtnBottom: Color,
    val secBtnBorder: Color,
    val secBtnFg: Color,
    val fabTop: Color,
    val fabBottom: Color,
    val fabBorder: Color,
    val fabFg: Color,
    val sasTop: Color,
    val sasBottom: Color,
    val sasBorder: Color,
    val sasCodeFg: Color,
    val switchOffBg: Color,
)

private val LightGlassPalette = GlassPalette(
    isDark = false,
    shellTop = Color(0xFFF6F9FF).copy(alpha = 0.56f),
    shellBottom = Color(0xFFEAF0FA).copy(alpha = 0.48f),
    shellBorder = Color.White.copy(alpha = 0.85f),
    sidebarTop = Color.White.copy(alpha = 0.68f),
    sidebarBottom = Color(0xFFF0F4FC).copy(alpha = 0.54f),
    sidebarBorder = Color(0xFF0F172A).copy(alpha = 0.09f),
    navIdleTop = Color.White.copy(alpha = 0.58f),
    navIdleBottom = Color.White.copy(alpha = 0.36f),
    navIdleBorder = Color(0xFF0F172A).copy(alpha = 0.12f),
    navActiveTop = Color.White.copy(alpha = 0.96f),
    navActiveBottom = Color(0xFFF4F9FF).copy(alpha = 0.84f),
    navActiveBorder = Color.White,
    cardTop = Color.White.copy(alpha = 0.84f),
    cardBottom = Color(0xFFF6F9FF).copy(alpha = 0.68f),
    cardBorder = Color.White.copy(alpha = 0.92f),
    specularLine = Color.White.copy(alpha = 0.95f),
    subRowBg = Color.White.copy(alpha = 0.62f),
    subRowBorder = Color(0xFF0F172A).copy(alpha = 0.08f),
    canvasTop = Color.White.copy(alpha = 0.72f),
    canvasBottom = Color(0xFFF2F6FE).copy(alpha = 0.58f),
    modalTop = Color.White.copy(alpha = 0.92f),
    modalBottom = Color(0xFFF4F7FE).copy(alpha = 0.84f),
    modalBorder = Color.White,
    modalScrim = Color(0xFF0F172A).copy(alpha = 0.24f),
    segWrapBg = Color(0xFF0F172A).copy(alpha = 0.08f),
    segActiveBg = Color.White,
    segActiveFg = Color(0xFF111319),
    textPrimary = Color(0xFF0F172A),
    textSecondary = Color(0xFF0F172A).copy(alpha = 0.72f),
    textMono = Color(0xFF0F172A).copy(alpha = 0.68f),
    metricFast = Color(0xFF16A34A).copy(alpha = 0.84f),
    metricWarm = Color(0xFFEAB308).copy(alpha = 0.88f),
    metricSlow = Color(0xFFF43F5E).copy(alpha = 0.84f),
    metricMuted = Color(0xFF64748B).copy(alpha = 0.50f),
    pillOnlineBg = Color(0xFF4ADE80).copy(alpha = 0.24f),
    pillOnlineBorder = Color(0xFF22C55E).copy(alpha = 0.36f),
    pillOnlineFg = Color(0xFF15803D),
    pillOfflineBg = Color(0xFF64748B).copy(alpha = 0.16f),
    pillOfflineBorder = Color(0xFF64748B).copy(alpha = 0.24f),
    pillOfflineFg = Color(0xFF475569),
    pillAmberBg = Color(0xFFF59E0B).copy(alpha = 0.20f),
    pillAmberBorder = Color(0xFFF59E0B).copy(alpha = 0.36f),
    pillAmberFg = Color(0xFFB45309),
    pillInfoBg = Color.White.copy(alpha = 0.72f),
    pillInfoBorder = Color(0xFF0F172A).copy(alpha = 0.10f),
    pillInfoFg = Color(0xFF0F172A).copy(alpha = 0.78f),
    pillDangerBg = Color(0xFFEF4444).copy(alpha = 0.15f),
    pillDangerBorder = Color(0xFFEF4444).copy(alpha = 0.30f),
    pillDangerFg = Color(0xFFDC2626),
    secBtnTop = Color.White.copy(alpha = 0.92f),
    secBtnBottom = Color(0xFFF2F6FE).copy(alpha = 0.76f),
    secBtnBorder = Color(0xFF0F172A).copy(alpha = 0.12f),
    secBtnFg = Color(0xFF161820),
    fabTop = Color.White.copy(alpha = 0.96f),
    fabBottom = Color(0xFFECF2FC).copy(alpha = 0.88f),
    fabBorder = Color.White,
    fabFg = Color(0xFF111319),
    sasTop = Color(0xFFFFFBEB).copy(alpha = 0.90f),
    sasBottom = Color(0xFFFEF3C7).copy(alpha = 0.74f),
    sasBorder = Color(0xFFF59E0B).copy(alpha = 0.44f),
    sasCodeFg = Color(0xFF2563EB),
    switchOffBg = Color(0xFF0F172A).copy(alpha = 0.16f),
)

private val DarkGlassPalette = GlassPalette(
    isDark = true,
    shellTop = Color(0xFF141926).copy(alpha = 0.66f),
    shellBottom = Color(0xFF0E121C).copy(alpha = 0.58f),
    shellBorder = Color.White.copy(alpha = 0.20f),
    sidebarTop = Color(0xFF202738).copy(alpha = 0.72f),
    sidebarBottom = Color(0xFF141926).copy(alpha = 0.60f),
    sidebarBorder = Color.White.copy(alpha = 0.11f),
    navIdleTop = Color.White.copy(alpha = 0.09f),
    navIdleBottom = Color.White.copy(alpha = 0.04f),
    navIdleBorder = Color.White.copy(alpha = 0.12f),
    navActiveTop = Color.White.copy(alpha = 0.24f),
    navActiveBottom = Color.White.copy(alpha = 0.13f),
    navActiveBorder = Color.White.copy(alpha = 0.30f),
    cardTop = Color(0xFF283044).copy(alpha = 0.78f),
    cardBottom = Color(0xFF1A202E).copy(alpha = 0.64f),
    cardBorder = Color.White.copy(alpha = 0.18f),
    specularLine = Color.White.copy(alpha = 0.18f),
    subRowBg = Color.White.copy(alpha = 0.08f),
    subRowBorder = Color.White.copy(alpha = 0.12f),
    canvasTop = Color(0xFF262D40).copy(alpha = 0.68f),
    canvasBottom = Color(0xFF181D2B).copy(alpha = 0.54f),
    modalTop = Color(0xFF222838).copy(alpha = 0.92f),
    modalBottom = Color(0xFF161A26).copy(alpha = 0.86f),
    modalBorder = Color.White.copy(alpha = 0.22f),
    modalScrim = Color.Black.copy(alpha = 0.50f),
    segWrapBg = Color.White.copy(alpha = 0.10f),
    segActiveBg = Color.White.copy(alpha = 0.24f),
    segActiveFg = Color.White,
    textPrimary = Color(0xFFF1F5F9),
    textSecondary = Color(0xFFF1F5F9).copy(alpha = 0.72f),
    textMono = Color(0xFFF1F5F9).copy(alpha = 0.68f),
    metricFast = Color(0xFF6EE7B7).copy(alpha = 0.88f),
    metricWarm = Color(0xFFFDE047).copy(alpha = 0.90f),
    metricSlow = Color(0xFFF43F5E).copy(alpha = 0.86f),
    metricMuted = Color(0xFF94A3B8).copy(alpha = 0.46f),
    pillOnlineBg = Color(0xFF4ADE80).copy(alpha = 0.20f),
    pillOnlineBorder = Color(0xFF4ADE80).copy(alpha = 0.36f),
    pillOnlineFg = Color(0xFF6EE7B7),
    pillOfflineBg = Color(0xFF94A3B8).copy(alpha = 0.18f),
    pillOfflineBorder = Color(0xFF94A3B8).copy(alpha = 0.28f),
    pillOfflineFg = Color(0xFFCBD5E1),
    pillAmberBg = Color(0xFFF59E0B).copy(alpha = 0.22f),
    pillAmberBorder = Color(0xFFF59E0B).copy(alpha = 0.38f),
    pillAmberFg = Color(0xFFFCD34D),
    pillInfoBg = Color.White.copy(alpha = 0.12f),
    pillInfoBorder = Color.White.copy(alpha = 0.18f),
    pillInfoFg = Color(0xFFF1F5F9).copy(alpha = 0.86f),
    pillDangerBg = Color(0xFFEF4444).copy(alpha = 0.20f),
    pillDangerBorder = Color(0xFFEF4444).copy(alpha = 0.36f),
    pillDangerFg = Color(0xFFFCA5A5),
    secBtnTop = Color.White.copy(alpha = 0.18f),
    secBtnBottom = Color.White.copy(alpha = 0.09f),
    secBtnBorder = Color.White.copy(alpha = 0.20f),
    secBtnFg = Color(0xFFEEF1F7),
    fabTop = Color(0xFF404A62).copy(alpha = 0.94f),
    fabBottom = Color(0xFF283042).copy(alpha = 0.88f),
    fabBorder = Color.White.copy(alpha = 0.28f),
    fabFg = Color.White,
    sasTop = Color(0xFF453111).copy(alpha = 0.78f),
    sasBottom = Color(0xFF30220C).copy(alpha = 0.68f),
    sasBorder = Color(0xFFFBBF24).copy(alpha = 0.38f),
    sasCodeFg = Color(0xFF7CB5FF),
    switchOffBg = Color.White.copy(alpha = 0.18f),
)

val LocalGlassPalette = staticCompositionLocalOf { LightGlassPalette }

@Composable
fun WorkspaceGlassTheme(
    themeMode: GlassThemeMode,
    content: @Composable () -> Unit,
) {
    val systemDark = isSystemInDarkTheme()
    val useDark = when (themeMode) {
        GlassThemeMode.LIGHT -> false
        GlassThemeMode.DARK -> true
        GlassThemeMode.SYSTEM -> systemDark
    }
    val palette = if (useDark) DarkGlassPalette else LightGlassPalette
    CompositionLocalProvider(LocalGlassPalette provides palette, content = content)
}

@Composable
fun FrostedWallpaperLayer(
    preset: GlassWallpaperPreset,
    customFrosted: ImageBitmap?,
    palette: GlassPalette,
    modifier: Modifier = Modifier,
) {
    Crossfade(
        targetState = preset to customFrosted,
        animationSpec = tween(durationMillis = 320),
        modifier = modifier,
        label = "wallpaperCrossfade",
    ) { (activePreset, customImg) ->
        Box(modifier = Modifier.fillMaxSize()) {
            if (activePreset == GlassWallpaperPreset.CUSTOM && customImg != null) {
                Image(
                    bitmap = customImg,
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.fillMaxSize(),
                )
            } else {
                val effectivePreset = if (activePreset == GlassWallpaperPreset.CUSTOM) {
                    GlassWallpaperPreset.AURORA
                } else {
                    activePreset
                }
                PresetBackdropCanvas(
                    preset = effectivePreset,
                    isDark = palette.isDark,
                    modifier = Modifier.fillMaxSize(),
                )
            }
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(Brush.linearGradient(listOf(palette.shellTop, palette.shellBottom))),
            )
        }
    }
}

@Composable
private fun PresetBackdropCanvas(
    preset: GlassWallpaperPreset,
    isDark: Boolean,
    modifier: Modifier = Modifier,
) {
    Box(
        modifier = modifier.drawBehind {
            val (bgTop, bgBot, orb1, orb2, orb3) = when (preset) {
                GlassWallpaperPreset.AURORA -> if (isDark) {
                    listOf(
                        Color(0xFF0E1729),
                        Color(0xFF102032),
                        Color(0xFF3B82F6).copy(alpha = 0.45f),
                        Color(0xFF10B981).copy(alpha = 0.34f),
                        Color(0xFF6366F1).copy(alpha = 0.32f),
                    )
                } else {
                    listOf(
                        Color(0xFFDDEAFE),
                        Color(0xFFE5F6F1),
                        Color(0xFF60A5FA).copy(alpha = 0.55f),
                        Color(0xFF34D399).copy(alpha = 0.42f),
                        Color(0xFFA78BFA).copy(alpha = 0.34f),
                    )
                }
                GlassWallpaperPreset.SUNSET -> if (isDark) {
                    listOf(
                        Color(0xFF211324),
                        Color(0xFF17162B),
                        Color(0xFFFB7185).copy(alpha = 0.42f),
                        Color(0xFFF59E0B).copy(alpha = 0.32f),
                        Color(0xFF8B5CF6).copy(alpha = 0.34f),
                    )
                } else {
                    listOf(
                        Color(0xFFFCE7F3),
                        Color(0xFFFEF3C7),
                        Color(0xFFFB7185).copy(alpha = 0.48f),
                        Color(0xFFFBBF24).copy(alpha = 0.44f),
                        Color(0xFFA78BFA).copy(alpha = 0.34f),
                    )
                }
                GlassWallpaperPreset.OCEAN, GlassWallpaperPreset.CUSTOM -> if (isDark) {
                    listOf(
                        Color(0xFF0A192F),
                        Color(0xFF0B2738),
                        Color(0xFF0EA5E9).copy(alpha = 0.44f),
                        Color(0xFF14B8A6).copy(alpha = 0.36f),
                        Color(0xFF2563EB).copy(alpha = 0.34f),
                    )
                } else {
                    listOf(
                        Color(0xFFD2F1FF),
                        Color(0xFFD8F7F2),
                        Color(0xFF38BDF8).copy(alpha = 0.52f),
                        Color(0xFF2DD4BF).copy(alpha = 0.44f),
                        Color(0xFF3B82F6).copy(alpha = 0.32f),
                    )
                }
            }

            drawRect(Brush.linearGradient(listOf(bgTop, bgBot)))

            val c1 = Offset(size.width * 0.18f, size.height * 0.16f)
            val r1 = size.maxDimension * 0.56f
            drawCircle(
                brush = Brush.radialGradient(listOf(orb1, Color.Transparent), center = c1, radius = r1),
                radius = r1,
                center = c1,
            )

            val c2 = Offset(size.width * 0.84f, size.height * 0.78f)
            val r2 = size.maxDimension * 0.58f
            drawCircle(
                brush = Brush.radialGradient(listOf(orb2, Color.Transparent), center = c2, radius = r2),
                radius = r2,
                center = c2,
            )

            val c3 = Offset(size.width * 0.55f, size.height * 0.45f)
            val r3 = size.maxDimension * 0.46f
            drawCircle(
                brush = Brush.radialGradient(listOf(orb3, Color.Transparent), center = c3, radius = r3),
                radius = r3,
                center = c3,
            )
        },
    )
}

@Composable
fun GlassText(
    text: String,
    modifier: Modifier = Modifier,
    color: Color = LocalGlassPalette.current.textPrimary,
    fontSizeSp: Int = 14,
    weight: FontWeight = FontWeight.Normal,
    mono: Boolean = false,
    letterSpacing: TextUnit = TextUnit.Unspecified,
    maxLines: Int = Int.MAX_VALUE,
    softWrap: Boolean = true,
    overflow: TextOverflow = TextOverflow.Clip,
) {
    BasicText(
        text = text,
        modifier = modifier,
        maxLines = maxLines,
        softWrap = softWrap,
        overflow = overflow,
        style = TextStyle(
            color = color,
            fontSize = fontSizeSp.sp,
            fontWeight = weight,
            fontFamily = if (mono) FontFamily.Monospace else FontFamily.Default,
            letterSpacing = letterSpacing,
        ),
    )
}

fun Modifier.topSpecularHighlight(lineColor: Color, cornerRadiusDp: Dp): Modifier = drawWithContent {
    drawContent()
    val r = cornerRadiusDp.toPx()
    val y = 1.dp.toPx()
    drawLine(
        color = lineColor,
        start = Offset(r * 0.45f, y),
        end = Offset(size.width - r * 0.45f, y),
        strokeWidth = 1.dp.toPx(),
    )
}

@Composable
fun GlassCard(
    modifier: Modifier = Modifier,
    cornerRadius: Dp = 22.dp,
    padding: PaddingValues = PaddingValues(18.dp),
    topColor: Color = LocalGlassPalette.current.cardTop,
    bottomColor: Color = LocalGlassPalette.current.cardBottom,
    borderColor: Color = LocalGlassPalette.current.cardBorder,
    spacing: Dp = 10.dp,
    content: @Composable ColumnScope.() -> Unit,
) {
    val g = LocalGlassPalette.current
    val shape = RoundedCornerShape(cornerRadius)
    Column(
        modifier = modifier
            .fillMaxWidth()
            .clip(shape)
            .background(Brush.linearGradient(listOf(topColor, bottomColor)))
            .border(1.dp, borderColor, shape)
            .topSpecularHighlight(g.specularLine, cornerRadius)
            .padding(padding),
        verticalArrangement = Arrangement.spacedBy(spacing),
        content = content,
    )
}

@Composable
fun GlassSubRow(
    modifier: Modifier = Modifier,
    padding: PaddingValues = PaddingValues(horizontal = 14.dp, vertical = 12.dp),
    content: @Composable ColumnScope.() -> Unit,
) {
    val g = LocalGlassPalette.current
    val shape = RoundedCornerShape(16.dp)
    Column(
        modifier = modifier
            .fillMaxWidth()
            .clip(shape)
            .background(g.subRowBg)
            .border(1.dp, g.subRowBorder, shape)
            .padding(padding),
        verticalArrangement = Arrangement.spacedBy(6.dp),
        content = content,
    )
}

@Composable
fun GlassNavCapsule(
    label: String,
    icon: GlassIconKind,
    active: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val g = LocalGlassPalette.current
    val shape = RoundedCornerShape(15.dp)
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val pressScale by animateFloatAsState(
        targetValue = if (pressed) 0.97f else 1f,
        animationSpec = spring(dampingRatio = 0.75f, stiffness = 620f),
        label = "navScale",
    )
    val topCol by animateColorAsState(
        targetValue = if (active) g.navActiveTop else g.navIdleTop,
        animationSpec = tween(durationMillis = 180),
        label = "navTop",
    )
    val botCol by animateColorAsState(
        targetValue = if (active) g.navActiveBottom else g.navIdleBottom,
        animationSpec = tween(durationMillis = 180),
        label = "navBot",
    )
    val borderCol by animateColorAsState(
        targetValue = if (active) g.navActiveBorder else g.navIdleBorder,
        animationSpec = tween(durationMillis = 180),
        label = "navBorder",
    )

    Row(
        modifier = modifier
            .fillMaxWidth()
            .height(46.dp)
            .graphicsLayer {
                scaleX = pressScale
                scaleY = pressScale
            }
            .clip(shape)
            .background(Brush.verticalGradient(listOf(topCol, botCol)))
            .border(1.dp, borderCol, shape)
            .clickable(interactionSource = interaction, indication = null, onClick = onClick)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        GlassNavBadgeIcon(kind = icon, size = 28.dp)
        GlassText(
            text = label,
            color = g.textPrimary,
            fontSizeSp = 13,
            weight = if (active) FontWeight.Bold else FontWeight.SemiBold,
            maxLines = 1,
            softWrap = false,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
fun GlassButton(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    variant: GlassButtonVariant = GlassButtonVariant.SECONDARY,
    enabled: Boolean = true,
    icon: GlassIconKind? = null,
    iconRotationDeg: Float = 0f,
    padding: PaddingValues = PaddingValues(horizontal = 15.dp, vertical = 8.dp),
) {
    val g = LocalGlassPalette.current
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(
        targetValue = if (pressed && enabled) 0.96f else 1f,
        animationSpec = spring(dampingRatio = 0.72f, stiffness = 650f),
        label = "btnScale",
    )
    val bgBrush = when (variant) {
        GlassButtonVariant.PRIMARY -> Brush.verticalGradient(listOf(Color(0xFF3B82F6), Color(0xFF2563EB)))
        GlassButtonVariant.SECONDARY -> Brush.verticalGradient(listOf(g.secBtnTop, g.secBtnBottom))
        GlassButtonVariant.DANGER -> Brush.verticalGradient(listOf(g.pillDangerBg, g.pillDangerBg))
    }
    val border = when (variant) {
        GlassButtonVariant.PRIMARY -> Color.White.copy(alpha = 0.30f)
        GlassButtonVariant.SECONDARY -> g.secBtnBorder
        GlassButtonVariant.DANGER -> g.pillDangerBorder
    }
    val fg = when (variant) {
        GlassButtonVariant.PRIMARY -> Color.White
        GlassButtonVariant.SECONDARY -> g.secBtnFg
        GlassButtonVariant.DANGER -> g.pillDangerFg
    }
    val effFg = if (enabled) fg else fg.copy(alpha = 0.45f)
    Row(
        modifier = modifier
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .clip(CircleShape)
            .background(bgBrush)
            .border(1.dp, border, CircleShape)
            .then(
                if (enabled) {
                    Modifier.clickable(
                        interactionSource = interaction,
                        indication = null,
                        onClick = onClick,
                    )
                } else {
                    Modifier
                },
            )
            .padding(padding),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
    ) {
        if (icon != null) {
            GlassIcon(
                kind = icon,
                tint = effFg,
                size = 14.dp,
                modifier = if (iconRotationDeg != 0f) {
                    Modifier.graphicsLayer { rotationZ = iconRotationDeg }
                } else {
                    Modifier
                },
            )
        }
        GlassText(
            text = label,
            color = effFg,
            fontSizeSp = 12,
            weight = FontWeight.SemiBold,
            maxLines = 1,
            softWrap = false,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
fun GlassStatusPill(
    label: String,
    tone: GlassPillTone,
    modifier: Modifier = Modifier,
    dot: Boolean = false,
) {
    val g = LocalGlassPalette.current
    val (bg, border, fg) = when (tone) {
        GlassPillTone.ONLINE -> Triple(g.pillOnlineBg, g.pillOnlineBorder, g.pillOnlineFg)
        GlassPillTone.OFFLINE -> Triple(g.pillOfflineBg, g.pillOfflineBorder, g.pillOfflineFg)
        GlassPillTone.AMBER -> Triple(g.pillAmberBg, g.pillAmberBorder, g.pillAmberFg)
        GlassPillTone.INFO -> Triple(g.pillInfoBg, g.pillInfoBorder, g.pillInfoFg)
        GlassPillTone.DANGER -> Triple(g.pillDangerBg, g.pillDangerBorder, g.pillDangerFg)
    }
    Row(
        modifier = modifier
            .clip(CircleShape)
            .background(bg)
            .border(1.dp, border, CircleShape)
            .padding(horizontal = 10.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        if (dot) {
            Box(
                modifier = Modifier
                    .size(6.dp)
                    .clip(CircleShape)
                    .background(fg),
            )
        }
        GlassText(
            text = label,
            color = fg,
            fontSizeSp = 11,
            weight = FontWeight.Bold,
            maxLines = 1,
            softWrap = false,
        )
    }
}

@Composable
fun GlassSwitch(
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
) {
    val g = LocalGlassPalette.current
    val trackColor by animateColorAsState(
        targetValue = if (checked) Color(0xFF2563EB) else g.switchOffBg,
        animationSpec = tween(durationMillis = 180),
        label = "switchTrack",
    )
    val thumbOffset by animateDpAsState(
        targetValue = if (checked) 22.dp else 3.dp,
        animationSpec = spring(dampingRatio = 0.72f, stiffness = 520f),
        label = "switchThumb",
    )
    Box(
        modifier = modifier
            .width(48.dp)
            .height(28.dp)
            .clip(CircleShape)
            .background(trackColor)
            .border(1.dp, Color.White.copy(alpha = if (checked) 0.25f else 0.35f), CircleShape)
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
            ) { onCheckedChange(!checked) },
        contentAlignment = Alignment.CenterStart,
    ) {
        Box(
            modifier = Modifier
                .offset(x = thumbOffset)
                .size(22.dp)
                .clip(CircleShape)
                .background(Color.White),
        )
    }
}

@Composable
fun GlassTextField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    onDone: (() -> Unit)? = null,
) {
    val g = LocalGlassPalette.current
    var focused by remember { mutableStateOf(false) }
    val borderCol by animateColorAsState(
        targetValue = if (focused) Color(0xFF3B82F6) else g.subRowBorder,
        animationSpec = tween(durationMillis = 160),
        label = "fieldBorder",
    )
    val shape = RoundedCornerShape(12.dp)
    Box(
        modifier = modifier
            .height(40.dp)
            .clip(shape)
            .background(g.segWrapBg)
            .border(if (focused) 1.5.dp else 1.dp, borderCol, shape)
            .padding(horizontal = 12.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        BasicTextField(
            value = value,
            onValueChange = { next ->
                val singleLine = next.replace("\r", "").replace("\n", "")
                if (singleLine.length <= 64) {
                    onValueChange(singleLine)
                }
            },
            singleLine = true,
            textStyle = TextStyle(
                color = g.textPrimary,
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
            ),
            cursorBrush = SolidColor(Color(0xFF3B82F6)),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { onDone?.invoke() }),
            modifier = Modifier
                .fillMaxWidth()
                .onFocusChanged { focused = it.isFocused },
            decorationBox = { innerTextField ->
                if (value.isEmpty()) {
                    GlassText(
                        text = placeholder,
                        color = g.textSecondary.copy(alpha = 0.65f),
                        fontSizeSp = 13,
                        maxLines = 1,
                        softWrap = false,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                innerTextField()
            },
        )
    }
}

@Composable
fun GlassNavBadgeIcon(
    kind: GlassIconKind,
    size: Dp = 28.dp,
    activeColor: Color = Color(0xFF0A84FF),
) {
    Box(modifier = Modifier.size(size), contentAlignment = Alignment.Center) {
        Canvas(modifier = Modifier.matchParentSize()) {
            val r = this.size.minDimension * 0.5f
            val center = Offset(this.size.width * 0.5f, this.size.height * 0.5f)
            drawCircle(
                brush = Brush.verticalGradient(
                    listOf(activeColor.copy(alpha = 0.92f), Color(0xFF0A78F2)),
                ),
                radius = r,
                center = center,
            )
            drawArc(
                color = Color.White.copy(alpha = 0.38f),
                startAngle = 205f,
                sweepAngle = 130f,
                useCenter = false,
                style = Stroke(width = 1.dp.toPx(), cap = StrokeCap.Round),
            )
        }
        GlassIcon(kind = kind, tint = Color.White, size = size * 0.56f)
    }
}

@Composable
fun GlassIcon(
    kind: GlassIconKind,
    tint: Color,
    size: Dp = 18.dp,
    modifier: Modifier = Modifier,
) {
    val drawableRes = when (kind) {
        GlassIconKind.REFRESH -> R.drawable.ic_glass_refresh
        GlassIconKind.SETTINGS -> R.drawable.ic_glass_settings
        else -> 0
    }
    if (drawableRes != 0) {
        Image(
            painter = painterResource(drawableRes),
            contentDescription = null,
            colorFilter = ColorFilter.tint(tint),
            modifier = modifier.size(size),
        )
    } else {
        Canvas(modifier = modifier.size(size)) {
            val d = this.size.minDimension
            drawVectorGlyph(kind, tint, d, (this.size.width - d) * 0.5f, (this.size.height - d) * 0.5f)
        }
    }
}

private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawVectorGlyph(
    kind: GlassIconKind,
    color: Color,
    boxPx: Float,
    ox: Float,
    oy: Float,
) {
    val s = boxPx / 24f
    val stroke = Stroke(width = 2.05f * s, cap = StrokeCap.Round, join = StrokeJoin.Round)
    fun pt(x: Float, y: Float) = Offset(ox + x * s, oy + y * s)

    when (kind) {
        GlassIconKind.DEVICES -> {
            drawRoundRect(
                color = color,
                topLeft = pt(6.5f, 3f),
                size = Size(11f * s, 18f * s),
                cornerRadius = CornerRadius(2f * s, 2f * s),
                style = stroke,
            )
            drawLine(color, pt(10.5f, 18f), pt(13.5f, 18f), stroke.width, StrokeCap.Round)
        }
        GlassIconKind.CLIPBOARD -> {
            drawRoundRect(
                color = color,
                topLeft = pt(5.5f, 5f),
                size = Size(13f * s, 16f * s),
                cornerRadius = CornerRadius(2f * s, 2f * s),
                style = stroke,
            )
            drawRoundRect(
                color = color,
                topLeft = pt(9f, 3f),
                size = Size(6f * s, 3.5f * s),
                cornerRadius = CornerRadius(1f * s, 1f * s),
                style = stroke,
            )
            drawLine(color, pt(8.5f, 11.5f), pt(15.5f, 11.5f), stroke.width, StrokeCap.Round)
            drawLine(color, pt(8.5f, 15.5f), pt(13.5f, 15.5f), stroke.width, StrokeCap.Round)
        }
        GlassIconKind.TRANSFER -> {
            val p = Path().apply {
                moveTo(pt(7f, 8f).x, pt(7f, 8f).y)
                lineTo(pt(17f, 8f).x, pt(17f, 8f).y)
                moveTo(pt(14f, 5f).x, pt(14f, 5f).y)
                lineTo(pt(17f, 8f).x, pt(17f, 8f).y)
                lineTo(pt(14f, 11f).x, pt(14f, 11f).y)
                moveTo(pt(17f, 16f).x, pt(17f, 16f).y)
                lineTo(pt(7f, 16f).x, pt(7f, 16f).y)
                moveTo(pt(10f, 13f).x, pt(10f, 13f).y)
                lineTo(pt(7f, 16f).x, pt(7f, 16f).y)
                lineTo(pt(10f, 19f).x, pt(10f, 19f).y)
            }
            drawPath(p, color, style = stroke)
        }
        GlassIconKind.SCREEN -> {
            drawRoundRect(
                color = color,
                topLeft = pt(3.5f, 4.5f),
                size = Size(17f * s, 11.5f * s),
                cornerRadius = CornerRadius(2f * s, 2f * s),
                style = stroke,
            )
            drawLine(color, pt(8f, 19.5f), pt(16f, 19.5f), stroke.width, StrokeCap.Round)
            drawLine(color, pt(12f, 16f), pt(12f, 19.5f), stroke.width, StrokeCap.Round)
        }
        GlassIconKind.NOTES -> {
            drawRoundRect(
                color = color,
                topLeft = pt(5.5f, 3.5f),
                size = Size(13f * s, 17f * s),
                cornerRadius = CornerRadius(2f * s, 2f * s),
                style = stroke,
            )
            drawLine(color, pt(8.5f, 8.5f), pt(15.5f, 8.5f), stroke.width, StrokeCap.Round)
            drawLine(color, pt(8.5f, 12f), pt(15.5f, 12f), stroke.width, StrokeCap.Round)
            drawLine(color, pt(8.5f, 15.5f), pt(13f, 15.5f), stroke.width, StrokeCap.Round)
        }
        GlassIconKind.PLUS -> {
            val w = 2.4f * s
            drawLine(color, pt(12f, 5.5f), pt(12f, 18.5f), w, StrokeCap.Round)
            drawLine(color, pt(5.5f, 12f), pt(18.5f, 12f), w, StrokeCap.Round)
        }
        GlassIconKind.CLOSE -> {
            val w = 2.5f * s
            drawLine(color, pt(6.5f, 6.5f), pt(17.5f, 17.5f), w, StrokeCap.Round)
            drawLine(color, pt(17.5f, 6.5f), pt(6.5f, 17.5f), w, StrokeCap.Round)
        }
        GlassIconKind.PUSH -> {
            val p = Path().apply {
                moveTo(pt(12f, 18f).x, pt(12f, 18f).y)
                lineTo(pt(12f, 6f).x, pt(12f, 6f).y)
                moveTo(pt(7.5f, 10.5f).x, pt(7.5f, 10.5f).y)
                lineTo(pt(12f, 6f).x, pt(12f, 6f).y)
                lineTo(pt(16.5f, 10.5f).x, pt(16.5f, 10.5f).y)
            }
            drawPath(p, color, style = stroke)
        }
        GlassIconKind.PULL -> {
            val p = Path().apply {
                moveTo(pt(12f, 6f).x, pt(12f, 6f).y)
                lineTo(pt(12f, 18f).x, pt(12f, 18f).y)
                moveTo(pt(7.5f, 13.5f).x, pt(7.5f, 13.5f).y)
                lineTo(pt(12f, 18f).x, pt(12f, 18f).y)
                lineTo(pt(16.5f, 13.5f).x, pt(16.5f, 13.5f).y)
            }
            drawPath(p, color, style = stroke)
        }
        GlassIconKind.IMAGE -> {
            drawRoundRect(
                color = color,
                topLeft = pt(3.5f, 4.5f),
                size = Size(17f * s, 15f * s),
                cornerRadius = CornerRadius(2.5f * s, 2.5f * s),
                style = stroke,
            )
            drawCircle(color, radius = 1.6f * s, center = pt(8.5f, 9.5f))
            val mountain = Path().apply {
                moveTo(pt(4.5f, 17f).x, pt(4.5f, 17f).y)
                lineTo(pt(9.5f, 12.5f).x, pt(9.5f, 12.5f).y)
                lineTo(pt(13.5f, 16f).x, pt(13.5f, 16f).y)
                lineTo(pt(16f, 13.5f).x, pt(16f, 13.5f).y)
                lineTo(pt(19.5f, 17f).x, pt(19.5f, 17f).y)
            }
            drawPath(mountain, color, style = stroke)
        }
        GlassIconKind.REFRESH, GlassIconKind.SETTINGS -> Unit
    }
}

fun formatHumanBytes(bytes: Long): String {
    if (bytes < 1024L) return "$bytes B"
    val kb = bytes / 1024.0
    if (kb < 1024.0) return String.format("%.1f KB", kb)
    val mb = kb / 1024.0
    if (mb < 1024.0) return String.format("%.1f MB", mb)
    return String.format("%.2f GB", mb / 1024.0)
}
