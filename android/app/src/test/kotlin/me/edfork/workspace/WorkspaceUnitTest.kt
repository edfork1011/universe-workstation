package me.edfork.workspace

import android.view.MotionEvent
import java.io.InputStreamReader
import java.nio.charset.StandardCharsets
import java.util.Properties
import me.edfork.workspace.clip.ClipboardImageProvider
import me.edfork.workspace.gesture.TwoFingerSwipeGestureDetector
import me.edfork.workspace.model.SwipeDirection
import me.edfork.workspace.model.WorkspaceContract
import me.edfork.workspace.model.WorkspaceSnapshot
import me.edfork.workspace.ui.blurArgbPixels
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WorkspaceUnitTest {

    @Test
    fun gestureGateIsStrictlyFalseUntilLanConnected() {
        val disconnected = WorkspaceSnapshot(
            daemonReachable = true,
            lanConnected = false,
            gestureEnabled = true,
            syncPaused = false,
        )
        assertFalse(disconnected.isGestureActive)

        val detector = TwoFingerSwipeGestureDetector(
            density = 3.0f,
            canActivate = { disconnected.isGestureActive },
        )
        val down = detector.observeRaw(
            MotionEvent.ACTION_POINTER_DOWN, 2,
            200f, 900f, 350f, 910f, 1000L, 1080, 2400,
        )
        assertEquals(TwoFingerSwipeGestureDetector.Result.Ignored, down)

        val move = detector.observeRaw(
            MotionEvent.ACTION_MOVE, 2,
            200f, 500f, 350f, 510f, 1100L, 1080, 2400,
        )
        assertEquals(TwoFingerSwipeGestureDetector.Result.Abandoned, move)
    }

    @Test
    fun gestureConfirmsPushUpAndPullDownWhenConnected() {
        val connected = WorkspaceSnapshot(
            daemonReachable = true,
            lanConnected = true,
            gestureEnabled = true,
            syncPaused = false,
        )
        assertTrue(connected.isGestureActive)

        val detector = TwoFingerSwipeGestureDetector(
            density = 3.0f,
            canActivate = { connected.isGestureActive },
        )
        assertEquals(
            TwoFingerSwipeGestureDetector.Result.Observing,
            detector.observeRaw(MotionEvent.ACTION_POINTER_DOWN, 2, 200f, 1000f, 360f, 1010f, 1000L, 1080, 2400),
        )
        val confirmedUp = detector.observeRaw(
            MotionEvent.ACTION_MOVE, 2,
            205f, 700f, 365f, 710f, 1120L, 1080, 2400,
        )
        assertTrue(confirmedUp is TwoFingerSwipeGestureDetector.Result.Confirmed)
        assertEquals(SwipeDirection.UP, (confirmedUp as TwoFingerSwipeGestureDetector.Result.Confirmed).direction)

        assertEquals(
            TwoFingerSwipeGestureDetector.Result.Observing,
            detector.observeRaw(MotionEvent.ACTION_POINTER_DOWN, 2, 200f, 700f, 360f, 710f, 2000L, 1080, 2400),
        )
        val confirmedDown = detector.observeRaw(
            MotionEvent.ACTION_MOVE, 2,
            205f, 1050f, 365f, 1060f, 2120L, 1080, 2400,
        )
        assertTrue(confirmedDown is TwoFingerSwipeGestureDetector.Result.Confirmed)
        assertEquals(SwipeDirection.DOWN, (confirmedDown as TwoFingerSwipeGestureDetector.Result.Confirmed).direction)
    }

    @Test
    fun snapshotJsonParsesLanConnectedAndOshareIdsCleanly() {
        val json = """
        {
          "device_id": "android123",
          "device_name": "OPPO Find",
          "fingerprint": "AA11-BB22",
          "tls_port": 45877,
          "lan_connected": true,
          "sync_paused": false,
          "gesture_enabled": true,
          "gesture_fingers": 2,
          "swipe_up_action": "push",
          "swipe_down_action": "pull",
          "receive_dir": "/storage/emulated/0/Download/寰宇工作台",
          "paired_peers": [
            {
              "device_id": "archpc001",
              "device_name": "Arch Workstation",
              "platform": "linux",
              "fingerprint": "1111-2222",
              "connected": true,
              "ip": "192.168.1.10",
              "tls_port": 45877,
              "rtt_ms": 4
            }
          ],
          "discovered_peers": [],
          "pending_pairs": [],
          "transfers": []
        }
        """.trimIndent()

        val snap = WorkspaceSnapshot.fromJson(json)
        assertTrue(snap.isGestureActive)
        assertEquals(1, snap.onlinePairedPeers.size)
        val mac = WorkspaceContract.oshareBleMac(snap.onlinePairedPeers[0].deviceId)
        assertEquals("workspace_lan:archpc001", mac)
        assertEquals("archpc001", WorkspaceContract.parseDeviceIdFromOshareMac(mac))
        assertNull(WorkspaceContract.parseDeviceIdFromOshareMac("mac_device_mac"))
    }

    @Test
    fun oshareVirtualMacRoundTripsRealLinuxDeviceId() {
        val realLinuxId = "ad8f20353ab2f83f6823b9aef2578c4e"
        val mac = WorkspaceContract.oshareBleMac(realLinuxId)
        assertTrue(mac.startsWith(WorkspaceContract.OSHARE_MAC_PREFIX))
        assertEquals(realLinuxId, WorkspaceContract.parseDeviceIdFromOshareMac(mac))
    }

    @Test
    fun boxBlurSmoothsSharpEdgeAndPreservesOpaqueAlpha() {
        val w = 32
        val h = 16
        val pixels = IntArray(w * h) { idx ->
            val x = idx % w
            if (x < w / 2) 0xFF000000.toInt() else 0xFFFFFFFF.toInt()
        }
        blurArgbPixels(pixels, w, h, radius = 4, passes = 3)

        val midLeft = (pixels[h / 2 * w + (w / 2 - 2)] ushr 16) and 0xFF
        val midRight = (pixels[h / 2 * w + (w / 2 + 1)] ushr 16) and 0xFF
        val alphaEdge = (pixels[h / 2 * w + w / 2] ushr 24) and 0xFF
        assertTrue("Edge pixel should smooth into intermediate gray, got left=$midLeft", midLeft in 10..200)
        assertTrue("Right boundary should be brighter than left, got left=$midLeft right=$midRight", midRight > midLeft)
        assertEquals(0xFF, alphaEdge)
    }

    @Test
    fun clipboardImageProviderValidatesPathsAndMimes() {
        assertEquals("clip_123_ab.png", ClipboardImageProvider.parseValidImageFileName("/image/clip_123_ab.png"))
        assertEquals("photo.jpeg", ClipboardImageProvider.parseValidImageFileName("/image/photo.jpeg"))
        assertNull(ClipboardImageProvider.parseValidImageFileName("/image/../secret.png"))
        assertNull(ClipboardImageProvider.parseValidImageFileName("/other/clip.png"))
        assertNull(ClipboardImageProvider.parseValidImageFileName("/image/clip.txt"))
        assertEquals("image/png", ClipboardImageProvider.mimeForFileName("a.PNG"))
        assertEquals("image/webp", ClipboardImageProvider.mimeForFileName("a.webp"))
        assertEquals("image/jpeg", ClipboardImageProvider.mimeForFileName("a.jpg"))
    }

    @Test
    fun snapshotParsesUserNameDefaultAndCustomDeviceNameFields() {
        val defaultJson = """{"device_id":"abc","device_name":"OPPO Find N6"}"""
        val defaultSnap = WorkspaceSnapshot.fromJson(defaultJson)
        assertEquals("User", defaultSnap.userName)
        assertEquals("OPPO Find N6", defaultSnap.deviceName)
        assertFalse(defaultSnap.deviceNameCustomized)

        val customJson = """{"device_id":"abc","device_name":"Alice 的 OnePlus Open","device_name_customized":true,"user_name":"Alice"}"""
        val customSnap = WorkspaceSnapshot.fromJson(customJson)
        assertEquals("Alice", customSnap.userName)
        assertEquals("Alice 的 OnePlus Open", customSnap.deviceName)
        assertTrue(customSnap.deviceNameCustomized)
    }

    @Test
    fun unifiedVersionMatchesBuildConfigAndXposedModuleProp() {
        assertEquals("1.0-ant", BuildConfig.VERSION_NAME)
        assertEquals("1.0 Ant", BuildConfig.VERSION_DISPLAY)
        assertEquals(100, BuildConfig.VERSION_CODE)

        val stream = javaClass.classLoader!!.getResourceAsStream("META-INF/xposed/module.prop")
        requireNotNull(stream) { "META-INF/xposed/module.prop resource missing" }
        val props = Properties().apply {
            stream.use { load(InputStreamReader(it, StandardCharsets.UTF_8)) }
        }
        assertEquals(BuildConfig.VERSION_NAME, props.getProperty("version"))
        assertEquals(BuildConfig.VERSION_CODE, props.getProperty("versionCode")?.toInt())
    }
}
