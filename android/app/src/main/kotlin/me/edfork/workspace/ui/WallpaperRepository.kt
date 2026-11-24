package me.edfork.workspace.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import java.io.File
import java.io.FileOutputStream
import kotlin.math.max
import kotlin.math.roundToInt

enum class GlassWallpaperPreset(val prefValue: String, val label: String) {
    AURORA("aurora", "极光"),
    SUNSET("sunset", "绯霞"),
    OCEAN("ocean", "深海"),
    CUSTOM("custom", "自定义");

    companion object {
        fun fromPref(raw: String?): GlassWallpaperPreset =
            entries.firstOrNull { it.prefValue == raw } ?: AURORA
    }
}

/**
 * Fast O(N) sliding-window separable multi-pass box blur on ARGB pixels.
 * Preserves full 0xFF opaque alpha so cached frosted wallpapers require zero runtime blur passes.
 */
fun blurArgbPixels(
    pixels: IntArray,
    width: Int,
    height: Int,
    radius: Int,
    passes: Int = 3,
) {
    if (width <= 0 || height <= 0 || radius <= 0 || passes <= 0 || pixels.size < width * height) {
        return
    }
    val tmp = IntArray(width * height)
    val div = radius * 2 + 1

    repeat(passes) {
        // Horizontal sliding-window pass: pixels -> tmp
        for (y in 0 until height) {
            val rowOff = y * width
            var sumR = 0
            var sumG = 0
            var sumB = 0
            for (k in -radius..radius) {
                val sx = k.coerceIn(0, width - 1)
                val p = pixels[rowOff + sx]
                sumR += (p ushr 16) and 0xFF
                sumG += (p ushr 8) and 0xFF
                sumB += p and 0xFF
            }
            for (x in 0 until width) {
                tmp[rowOff + x] = (0xFF shl 24) or
                    ((sumR / div) shl 16) or
                    ((sumG / div) shl 8) or
                    (sumB / div)

                val outX = (x - radius).coerceIn(0, width - 1)
                val inX = (x + radius + 1).coerceIn(0, width - 1)
                val pOut = pixels[rowOff + outX]
                val pIn = pixels[rowOff + inX]
                sumR += ((pIn ushr 16) and 0xFF) - ((pOut ushr 16) and 0xFF)
                sumG += ((pIn ushr 8) and 0xFF) - ((pOut ushr 8) and 0xFF)
                sumB += (pIn and 0xFF) - (pOut and 0xFF)
            }
        }

        // Vertical sliding-window pass: tmp -> pixels
        for (x in 0 until width) {
            var sumR = 0
            var sumG = 0
            var sumB = 0
            for (k in -radius..radius) {
                val sy = k.coerceIn(0, height - 1)
                val p = tmp[sy * width + x]
                sumR += (p ushr 16) and 0xFF
                sumG += (p ushr 8) and 0xFF
                sumB += p and 0xFF
            }
            for (y in 0 until height) {
                val idx = y * width + x
                pixels[idx] = (0xFF shl 24) or
                    ((sumR / div) shl 16) or
                    ((sumG / div) shl 8) or
                    (sumB / div)

                val outY = (y - radius).coerceIn(0, height - 1)
                val inY = (y + radius + 1).coerceIn(0, height - 1)
                val pOut = tmp[outY * width + x]
                val pIn = tmp[inY * width + x]
                sumR += ((pIn ushr 16) and 0xFF) - ((pOut ushr 16) and 0xFF)
                sumG += ((pIn ushr 8) and 0xFF) - ((pOut ushr 8) and 0xFF)
                sumB += (pIn and 0xFF) - (pOut and 0xFF)
            }
        }
    }
}

object WallpaperRepository {
    private const val FROSTED_FILE = "wallpaper_frosted.jpg"
    private const val THUMB_FILE = "wallpaper_thumb.jpg"
    private const val MAX_FROSTED_DIM = 540
    private const val MAX_THUMB_DIM = 160
    private const val BLUR_RADIUS = 18

    fun loadCachedBitmaps(context: Context): Pair<ImageBitmap?, ImageBitmap?> {
        val frostedFile = File(context.filesDir, FROSTED_FILE)
        val thumbFile = File(context.filesDir, THUMB_FILE)
        val frosted = if (frostedFile.exists()) {
            runCatching { BitmapFactory.decodeFile(frostedFile.absolutePath)?.asImageBitmap() }.getOrNull()
        } else {
            null
        }
        val thumb = if (thumbFile.exists()) {
            runCatching { BitmapFactory.decodeFile(thumbFile.absolutePath)?.asImageBitmap() }.getOrNull()
        } else {
            null
        }
        return frosted to thumb
    }

    fun importAndBlurUri(context: Context, uri: Uri): Result<Pair<ImageBitmap, ImageBitmap>> = runCatching {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        context.contentResolver.openInputStream(uri)?.use { stream ->
            BitmapFactory.decodeStream(stream, null, bounds)
        } ?: error("无法读取所选图片")

        val rawW = bounds.outWidth
        val rawH = bounds.outHeight
        require(rawW > 0 && rawH > 0) { "无效图片尺寸" }

        var sampleSize = 1
        while (max(rawW / sampleSize, rawH / sampleSize) > 1080) {
            sampleSize *= 2
        }

        val decodeOpts = BitmapFactory.Options().apply {
            inSampleSize = sampleSize
            inPreferredConfig = Bitmap.Config.ARGB_8888
        }
        val sampled = context.contentResolver.openInputStream(uri)?.use { stream ->
            BitmapFactory.decodeStream(stream, null, decodeOpts)
        } ?: error("图片解码失败")

        try {
            val baseArgb = if (sampled.config == Bitmap.Config.ARGB_8888) {
                sampled
            } else {
                sampled.copy(Bitmap.Config.ARGB_8888, false)
            }

            val frostedSrc = scaleToMaxDim(baseArgb, MAX_FROSTED_DIM)
            val fw = frostedSrc.width
            val fh = frostedSrc.height
            val pixels = IntArray(fw * fh)
            frostedSrc.getPixels(pixels, 0, fw, 0, 0, fw, fh)
            blurArgbPixels(pixels, fw, fh, radius = BLUR_RADIUS, passes = 3)
            val frostedOut = Bitmap.createBitmap(pixels, fw, fh, Bitmap.Config.ARGB_8888)

            val thumbOut = scaleToMaxDim(frostedOut, MAX_THUMB_DIM)

            FileOutputStream(File(context.filesDir, FROSTED_FILE)).use { out ->
                frostedOut.compress(Bitmap.CompressFormat.JPEG, 90, out)
            }
            FileOutputStream(File(context.filesDir, THUMB_FILE)).use { out ->
                thumbOut.compress(Bitmap.CompressFormat.JPEG, 88, out)
            }

            if (frostedSrc !== baseArgb) frostedSrc.recycle()
            if (baseArgb !== sampled) baseArgb.recycle()

            frostedOut.asImageBitmap() to thumbOut.asImageBitmap()
        } finally {
            if (!sampled.isRecycled) sampled.recycle()
        }
    }

    fun clearCustomWallpaper(context: Context) {
        runCatching { File(context.filesDir, FROSTED_FILE).delete() }
        runCatching { File(context.filesDir, THUMB_FILE).delete() }
    }

    private fun scaleToMaxDim(src: Bitmap, maxDim: Int): Bitmap {
        val w = src.width
        val h = src.height
        val longest = max(w, h)
        if (longest <= maxDim) return src
        val ratio = maxDim.toFloat() / longest.toFloat()
        val dstW = (w * ratio).roundToInt().coerceAtLeast(1)
        val dstH = (h * ratio).roundToInt().coerceAtLeast(1)
        return Bitmap.createScaledBitmap(src, dstW, dstH, true)
    }
}
