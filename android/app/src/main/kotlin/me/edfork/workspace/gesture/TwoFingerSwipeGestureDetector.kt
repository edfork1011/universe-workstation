package me.edfork.workspace.gesture

import android.view.MotionEvent
import kotlin.math.abs
import me.edfork.workspace.model.SwipeDirection

/**
 * Recognizes deliberate two-finger vertical swipes (`UP` / `DOWN`) while passing normal
 * single-finger touches, horizontal swipes, and pinch gestures untouched.
 */
class TwoFingerSwipeGestureDetector(
    private val density: Float,
    private val minimumDistanceDp: Int = 30,
    private val maxFingerGapDriftDp: Int = 60,
    private val canActivate: () -> Boolean = { true },
) {
    sealed interface Result {
        data object Ignored : Result
        data object Observing : Result
        data object Abandoned : Result
        data class Confirmed(
            val direction: SwipeDirection,
            val dxDp: Int,
            val dyDp: Int,
            val durationMs: Long,
        ) : Result {
            val description: String
                get() = "two-finger ${direction.name.lowercase()} (${dxDp},${dyDp}dp in ${durationMs}ms)"
        }
    }

    private val minimumDistancePx = minimumDistanceDp.coerceIn(16, 160) * density
    private val maxGapDriftPx = maxFingerGapDriftDp.coerceIn(24, 160) * density

    private var tracking = false
    private var downTime = 0L
    private var startCentroidX = 0f
    private var startCentroidY = 0f
    private var startGapY = 0f

    val isObserving: Boolean
        get() = tracking

    fun reset() {
        tracking = false
    }

    private fun dp(pixels: Float): Int = if (density > 0f) (pixels / density).toInt() else pixels.toInt()

    private fun rawX(event: MotionEvent, pointerIndex: Int): Float =
        event.getX(pointerIndex) + (event.rawX - event.getX(0))

    private fun rawY(event: MotionEvent, pointerIndex: Int): Float =
        event.getY(pointerIndex) + (event.rawY - event.getY(0))

    fun observe(event: MotionEvent, screenWidth: Int, screenHeight: Int): Result {
        val count = event.pointerCount
        val x0 = if (count >= 1) rawX(event, 0) else 0f
        val y0 = if (count >= 1) rawY(event, 0) else 0f
        val x1 = if (count >= 2) rawX(event, 1) else 0f
        val y1 = if (count >= 2) rawY(event, 1) else 0f
        return observeRaw(
            actionMasked = event.actionMasked,
            pointerCount = count,
            rawX0 = x0,
            rawY0 = y0,
            rawX1 = x1,
            rawY1 = y1,
            eventTime = event.eventTime,
            screenWidth = screenWidth,
            screenHeight = screenHeight,
        )
    }

    fun observeRaw(
        actionMasked: Int,
        pointerCount: Int,
        rawX0: Float,
        rawY0: Float,
        rawX1: Float,
        rawY1: Float,
        eventTime: Long,
        screenWidth: Int,
        screenHeight: Int,
    ): Result {
        when (actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                tracking = false
                return Result.Ignored
            }

            MotionEvent.ACTION_POINTER_DOWN -> {
                if (pointerCount != 2 || !canActivate()) {
                    tracking = false
                    return Result.Ignored
                }
                startCentroidX = (rawX0 + rawX1) / 2f
                startCentroidY = (rawY0 + rawY1) / 2f
                startGapY = abs(rawY0 - rawY1)
                if (!startCentroidX.isFinite() ||
                    !startCentroidY.isFinite() ||
                    screenWidth <= 0 ||
                    screenHeight <= 0 ||
                    startCentroidX !in 0f..screenWidth.toFloat() ||
                    startCentroidY !in 0f..screenHeight.toFloat()
                ) {
                    tracking = false
                    return Result.Ignored
                }
                downTime = eventTime
                tracking = true
                return Result.Observing
            }

            MotionEvent.ACTION_MOVE -> {
                if (!tracking || pointerCount != 2 || !canActivate()) {
                    tracking = false
                    return Result.Abandoned
                }
                val elapsedMs = eventTime - downTime
                if (elapsedMs !in 0L..MAX_DURATION_MS) {
                    tracking = false
                    return Result.Abandoned
                }
                val centroidX = (rawX0 + rawX1) / 2f
                val centroidY = (rawY0 + rawY1) / 2f
                val dx = centroidX - startCentroidX
                val dy = centroidY - startCentroidY
                if (!dx.isFinite() || !dy.isFinite()) {
                    tracking = false
                    return Result.Abandoned
                }
                if (abs(abs(rawY0 - rawY1) - startGapY) > maxGapDriftPx) {
                    tracking = false
                    return Result.Abandoned
                }
                val vertical = abs(dy)
                val horizontal = abs(dx)
                if (vertical >= minimumDistancePx && vertical > horizontal * DOMINANCE_RATIO) {
                    tracking = false
                    if (!canActivate()) return Result.Abandoned
                    val dir = if (dy < 0f) SwipeDirection.UP else SwipeDirection.DOWN
                    return Result.Confirmed(dir, dp(dx), dp(dy), elapsedMs)
                }
                if (horizontal >= minimumDistancePx && horizontal > vertical * DOMINANCE_RATIO) {
                    tracking = false
                    return Result.Abandoned
                }
                return Result.Observing
            }

            MotionEvent.ACTION_POINTER_UP,
            MotionEvent.ACTION_UP,
            MotionEvent.ACTION_CANCEL -> {
                val was = tracking
                tracking = false
                return if (was) Result.Abandoned else Result.Ignored
            }

            else -> return if (tracking) Result.Observing else Result.Ignored
        }
    }

    private companion object {
        const val MAX_DURATION_MS = 2_000L
        const val DOMINANCE_RATIO = 1.25f
    }
}
