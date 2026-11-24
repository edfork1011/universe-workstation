package me.edfork.workspace.systemui

import android.content.Context
import android.graphics.Point
import android.hardware.display.DisplayManager
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.os.VibrationEffect
import android.os.Vibrator
import android.view.Choreographer
import android.view.Display
import android.view.InputDevice
import android.view.MotionEvent
import java.lang.reflect.Proxy
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import me.edfork.workspace.gesture.TwoFingerSwipeGestureDetector
import me.edfork.workspace.logging.WorkspaceLog

/**
 * Installs raw input monitoring inside `com.android.systemui` ONLY when LAN connected.
 * Immediately disposes both InputMonitor and receiver on disconnect so unconnected touches pass
 * zero-overhead untouched.
 */
object SystemUiGestureMonitor {
    private const val COOLDOWN_MS = 550L

    private var monitor: Any? = null
    private var receiver: Any? = null
    private var displayManagerRef: DisplayManager? = null
    private var displayListenerRef: DisplayManager.DisplayListener? = null
    private var lastTriggerRealtimeMs = 0L

    val isInstalled: Boolean
        get() = receiver != null

    fun install(context: Context) {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            Handler(Looper.getMainLooper()).post { install(context) }
            return
        }
        if (receiver != null) return
        if (!SystemUiSwipeRuntime.currentSnapshot().isGestureActive) return

        var newMonitor: Any? = null
        var newReceiver: Any? = null
        var displays: DisplayManager? = null
        var listenerReg: DisplayManager.DisplayListener? = null

        runCatching {
            displays = context.getSystemService(DisplayManager::class.java)
            val display = displays?.getDisplay(Display.DEFAULT_DISPLAY)
                ?: error("Default display unavailable")
            val power = context.getSystemService(PowerManager::class.java)
            val vibrator = context.getSystemService(Vibrator::class.java)

            val managerCls = Class.forName("android.hardware.input.InputManagerGlobal")
            val manager = managerCls.getMethod("getInstance").invoke(null)
            val inputMonitor = managerCls.getMethod(
                "monitorGestureInput",
                String::class.java,
                Int::class.javaPrimitiveType,
            ).invoke(manager, "Workspace-lan-gesture", Display.DEFAULT_DISPLAY)
            newMonitor = inputMonitor
            val pilferMethod = inputMonitor.javaClass.getMethod("pilferPointers")

            val viewportSize = Point()
            var density = context.resources.displayMetrics.density
            var rotation = display.rotation

            val canActivate = {
                power?.isInteractive == true &&
                    SystemUiSwipeRuntime.currentSnapshot().isGestureActive
            }

            var detector = TwoFingerSwipeGestureDetector(
                density = density,
                canActivate = canActivate,
            )

            @Suppress("DEPRECATION")
            fun refreshViewport() {
                val updated = Point()
                display.getRealSize(updated)
                val updatedRot = display.rotation
                val updatedDen = context.resources.displayMetrics.density
                if (viewportSize != updated || rotation != updatedRot || density != updatedDen) {
                    viewportSize.set(updated.x, updated.y)
                    rotation = updatedRot
                    density = updatedDen
                    detector = TwoFingerSwipeGestureDetector(
                        density = density,
                        canActivate = canActivate,
                    )
                }
            }

            refreshViewport()

            val mainHandler = Handler(Looper.getMainLooper())
            listenerReg = object : DisplayManager.DisplayListener {
                override fun onDisplayAdded(displayId: Int) = Unit
                override fun onDisplayRemoved(displayId: Int) = Unit
                override fun onDisplayChanged(displayId: Int) {
                    if (displayId == Display.DEFAULT_DISPLAY) {
                        runCatching { refreshViewport() }.onFailure { detector.reset() }
                    }
                }
            }.also { displays.registerDisplayListener(it, mainHandler) }

            val receiverCls = Class.forName(
                "android.view.BatchedInputEventReceiver\$SimpleBatchedInputEventReceiver",
            )
            val listenerCls = Class.forName("${receiverCls.name}\$InputEventListener")
            val eventListener = Proxy.newProxyInstance(
                listenerCls.classLoader,
                arrayOf(listenerCls),
            ) { proxy, method, args ->
                when (method.name) {
                    "hashCode" -> System.identityHashCode(proxy)
                    "equals" -> proxy === args?.firstOrNull()
                    "toString" -> "WorkspaceGestureInputListener"
                    "onInputEvent" -> {
                        runCatching {
                            val event = args?.firstOrNull() as? MotionEvent
                            val active = SystemUiSwipeRuntime.currentSnapshot().isGestureActive
                            val relevantAction = event?.actionMasked == MotionEvent.ACTION_DOWN ||
                                event?.actionMasked == MotionEvent.ACTION_POINTER_DOWN ||
                                detector.isObserving

                            if (event == null ||
                                !active ||
                                !event.isFromSource(InputDevice.SOURCE_TOUCHSCREEN) ||
                                !relevantAction
                            ) {
                                detector.reset()
                                false
                            } else {
                                val res = detector.observe(event, viewportSize.x, viewportSize.y)
                                if (res is TwoFingerSwipeGestureDetector.Result.Confirmed) {
                                    val now = SystemClock.elapsedRealtime()
                                    if (now - lastTriggerRealtimeMs !in 0 until COOLDOWN_MS) {
                                        lastTriggerRealtimeMs = now
                                        runCatching { pilferMethod.invoke(inputMonitor) }
                                        runCatching {
                                            vibrator?.vibrate(
                                                VibrationEffect.createOneShot(
                                                    18L,
                                                    VibrationEffect.DEFAULT_AMPLITUDE,
                                                ),
                                            )
                                        }
                                        WorkspaceLog.info("gesture_monitor", "Confirmed ${res.description}")
                                        SystemUiSwipeRuntime.onConfirmedSwipe(res.direction)
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            }
                        }.getOrElse {
                            detector.reset()
                            false
                        }
                    }

                    else -> null
                }
            }

            newReceiver = receiverCls.getConstructor(
                Class.forName("android.view.InputChannel"),
                Looper::class.java,
                Choreographer::class.java,
                listenerCls,
            ).newInstance(
                inputMonitor.javaClass.getMethod("getInputChannel").invoke(inputMonitor),
                Looper.getMainLooper(),
                Choreographer.getInstance(),
                eventListener,
            )
            receiverCls.getMethod("setBatchingEnabled", Boolean::class.javaPrimitiveType)
                .invoke(newReceiver, false)

            monitor = inputMonitor
            receiver = newReceiver
            displayManagerRef = displays
            displayListenerRef = listenerReg
            WorkspaceLog.info("gesture_monitor", "Activated SystemUI gesture monitor (LAN connected)")
        }.onFailure { err ->
            listenerReg?.let { reg -> runCatching { displays?.unregisterDisplayListener(reg) } }
            for (r in listOf(newReceiver, newMonitor)) {
                runCatching { r?.javaClass?.getMethod("dispose")?.invoke(r) }
            }
            WorkspaceLog.error("gesture_monitor", "Failed installing gesture monitor", err)
        }
    }

    fun uninstall() {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            val latch = CountDownLatch(1)
            Handler(Looper.getMainLooper()).post {
                try {
                    uninstall()
                } finally {
                    latch.countDown()
                }
            }
            latch.await(2, TimeUnit.SECONDS)
            return
        }
        if (receiver == null && monitor == null) return

        displayListenerRef?.let { reg ->
            runCatching { displayManagerRef?.unregisterDisplayListener(reg) }
        }
        displayListenerRef = null
        displayManagerRef = null

        for (r in listOf(receiver, monitor)) {
            runCatching { r?.javaClass?.getMethod("dispose")?.invoke(r) }
        }
        receiver = null
        monitor = null
        WorkspaceLog.info("gesture_monitor", "Deactivated SystemUI gesture monitor (LAN disconnected or paused)")
    }
}
