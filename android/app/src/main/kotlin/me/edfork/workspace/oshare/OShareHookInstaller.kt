package me.edfork.workspace.oshare

import android.app.Application
import android.app.Instrumentation
import android.content.Context
import android.content.Intent
import android.util.SparseArray
import io.github.libxposed.api.XposedInterface
import io.github.libxposed.api.XposedModule
import me.edfork.workspace.logging.WorkspaceLog
import me.edfork.workspace.model.WorkspaceContract

/**
 * Hooks ColorOS `com.coloros.oshare` (`OShareClient`, `DirectShareServer`, `v9.a`)
 * to inject online paired Linux PCs into the native OPPO Share list and send files over LAN mTLS.
 */
object OShareHookInstaller {

    fun install(module: XposedModule, classLoader: ClassLoader) {
        hookBootstrap(module, classLoader)
        hookDeviceManager(module, classLoader)
        hookDirectShareServer(module, classLoader)
        hookOShareClient(module, classLoader)
        resolveExistingOShareContext(classLoader)?.let { ctx ->
            OShareLanRuntime.initialize(ctx, classLoader)
        }
    }

    private fun resolveExistingOShareContext(classLoader: ClassLoader): Context? {
        val fromOshare = runCatching {
            val appCls = classLoader.loadClass("com.oplus.oshare.OShareApplication")
            appCls.getMethod("h").invoke(null) as? Context
        }.getOrNull()
        if (fromOshare != null) return fromOshare

        return runCatching {
            val at = Class.forName("android.app.ActivityThread")
            at.getMethod("currentApplication").invoke(null) as? Context
        }.getOrNull()
    }

    private fun hookBootstrap(module: XposedModule, classLoader: ClassLoader) {
        runCatching {
            val method = Instrumentation::class.java.getDeclaredMethod(
                "callApplicationOnCreate",
                Application::class.java,
            )
            method.isAccessible = true
            module.hook(method)
                .setId("workspace_oshare_app_init")
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    val app = chain.getArg(0) as? Application
                    val res = chain.proceed()
                    if (app?.packageName == WorkspaceContract.OSHARE_PACKAGE) {
                        OShareLanRuntime.initialize(app, classLoader)
                    }
                    res
                }
        }.onFailure { err ->
            WorkspaceLog.warn("oshare_hooks", "Instrumentation hook failed", err)
        }
    }

    private fun hookDeviceManager(module: XposedModule, classLoader: ClassLoader) {
        val dmClass = runCatching { classLoader.loadClass("v9.a") }.getOrElse {
            WorkspaceLog.error("oshare_hooks", "Missing v9.a in com.coloros.oshare", it)
            return
        }

        // Preserve virtual LAN PC entries against timeout sweeps v9.a.x(String mac)
        runCatching {
            val removeMethod = dmClass.getDeclaredMethod("x", String::class.java)
            removeMethod.isAccessible = true
            module.hook(removeMethod)
                .setId("workspace_oshare_keep_virtual")
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    val mac = chain.getArg(0) as? String
                    if (OShareLanRuntime.isPeerOnline(mac)) {
                        return@intercept null
                    }
                    chain.proceed()
                }
        }

        // Re-inject online virtual LAN PCs whenever v9.a.g() clears transient BLE devices
        runCatching {
            val clearMethod = dmClass.getDeclaredMethod("g")
            clearMethod.isAccessible = true
            module.hook(clearMethod)
                .setId("workspace_oshare_reinject_after_clear")
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    val res = chain.proceed()
                    OShareLanRuntime.syncVirtualDevicesToManager()
                    res
                }
        }

        // Re-inject online virtual LAN PCs whenever v9.a.y() resets device states
        runCatching {
            val resetMethod = dmClass.getDeclaredMethod("y")
            resetMethod.isAccessible = true
            module.hook(resetMethod)
                .setId("workspace_oshare_reinject_after_reset")
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    val res = chain.proceed()
                    OShareLanRuntime.syncVirtualDevicesToManager()
                    res
                }
        }

        // Refresh TTL of online LAN PCs before the 1s v9.a$b timer task sweeps entries via Iterator.remove()
        runCatching {
            val timerTaskCls = classLoader.loadClass("v9.a\$b")
            val runMethod = timerTaskCls.getDeclaredMethod("run")
            runMethod.isAccessible = true
            module.hook(runMethod)
                .setId("workspace_oshare_timer_keepalive")
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    OShareLanRuntime.refreshVirtualDevicesTtl()
                    chain.proceed()
                }
        }.onFailure { err ->
            WorkspaceLog.warn("oshare_hooks", "Failed hooking v9.a\$b.run()", err)
        }

        // Ensure cold-start callers querying v9.a.n() immediately see paired online PCs
        runCatching {
            val getDevicesMethod = dmClass.getDeclaredMethod("n")
            getDevicesMethod.isAccessible = true
            module.hook(getDevicesMethod)
                .setId("workspace_oshare_query_sync")
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    OShareLanRuntime.ensureSyncedForDeviceQuery()
                    chain.proceed()
                }
        }.onFailure { err ->
            WorkspaceLog.warn("oshare_hooks", "Failed hooking v9.a.n()", err)
        }
    }

    private fun hookDirectShareServer(module: XposedModule, classLoader: ClassLoader) {
        val dssCls = runCatching {
            classLoader.loadClass("com.oplus.oshare.ui.direct.DirectShareServer")
        }.getOrElse {
            WorkspaceLog.warn("oshare_hooks", "DirectShareServer not present", it)
            return
        }
        val baseDevCls = runCatching { classLoader.loadClass("l9.b") }.getOrNull()

        if (baseDevCls != null) {
            for (methodName in listOf("b0", "c0")) {
                runCatching {
                    val m = dssCls.getDeclaredMethod(methodName, baseDevCls)
                    m.isAccessible = true
                    module.hook(m)
                        .setId("workspace_oshare_direct_allow_$methodName")
                        .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                        .intercept { chain ->
                            val bVar = chain.getArg(0)
                            val mac = runCatching {
                                bVar?.javaClass?.getMethod("a")?.invoke(bVar) as? String
                            }.getOrNull()
                            if (mac?.startsWith(WorkspaceContract.OSHARE_MAC_PREFIX) == true) {
                                return@intercept true
                            }
                            chain.proceed()
                        }
                }.onFailure { err ->
                    WorkspaceLog.warn("oshare_hooks", "Failed hooking DirectShareServer.$methodName", err)
                }
            }
        }

        // Allow DirectShare to remain active when Bluetooth is off if paired PC is online
        runCatching {
            val isReadyMethod = dssCls.getDeclaredMethod("J")
            isReadyMethod.isAccessible = true
            module.hook(isReadyMethod)
                .setId("workspace_oshare_direct_ready")
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    val orig = chain.proceed() as? Boolean ?: false
                    if (orig || !OShareLanRuntime.hasOnlinePeer()) return@intercept orig
                    val inst = chain.thisObject ?: return@intercept false
                    val bindFrom = runCatching {
                        val field = runCatching { dssCls.getDeclaredField("e") }
                            .getOrElse { dssCls.getDeclaredField("f11690e") }
                        field.isAccessible = true
                        field.getInt(inst)
                    }.getOrDefault(-1)
                    val zObj = runCatching {
                        dssCls.getMethod("Z").invoke(inst)
                    }.getOrNull() as? java.util.concurrent.atomic.AtomicBoolean
                    bindFrom > -1 && (zObj?.get() == true)
                }
        }

        // Ensure ShareActivity still receives device updates even if system Bluetooth is disabled
        runCatching {
            val j0Method = dssCls.getDeclaredMethod("j0", SparseArray::class.java, List::class.java)
            j0Method.isAccessible = true
            module.hook(j0Method)
                .setId("workspace_oshare_dispatch_bt_off")
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    val res = chain.proceed()
                    val inst = chain.thisObject
                    val sparse = chain.getArg(0) as? SparseArray<*>
                    val list = chain.getArg(1) as? List<*>
                    if (inst != null && sparse != null && list != null) {
                        OShareLanRuntime.dispatchWhenBtOffIfNeeded(inst, sparse, list)
                    }
                    res
                }
        }
    }

    private fun hookOShareClient(module: XposedModule, classLoader: ClassLoader) {
        val clientCls = runCatching {
            classLoader.loadClass("com.oplus.oshare.OShareClient")
        }.getOrElse {
            WorkspaceLog.error("oshare_hooks", "Missing OShareClient", it)
            return
        }

        // Keep OShare send switch open while paired PC is online on LAN
        runCatching {
            val jMethod = clientCls.getDeclaredMethod("J")
            jMethod.isAccessible = true
            module.hook(jMethod)
                .setId("workspace_oshare_client_j")
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    if (OShareLanRuntime.hasOnlinePeer()) return@intercept true
                    chain.proceed()
                }
        }

        val serviceImplCls = runCatching {
            classLoader.loadClass("com.oplus.oshare.OShareClient$6")
        }.getOrNull() ?: return
        val baseDevCls = runCatching { classLoader.loadClass("l9.b") }.getOrNull()

        runCatching {
            val isSendOn = serviceImplCls.getDeclaredMethod("isSendOn")
            isSendOn.isAccessible = true
            module.hook(isSendOn)
                .setId("workspace_oshare_is_send_on")
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    val orig = chain.proceed() as? Boolean ?: false
                    if (orig || OShareLanRuntime.hasOnlinePeer()) true else false
                }
        }

        for (method in serviceImplCls.declaredMethods) {
            if (method.name == "registerCallback" || method.name == "registerMultiCallback") {
                runCatching {
                    method.isAccessible = true
                    module.hook(method)
                        .setId("workspace_oshare_${method.name}")
                        .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                        .intercept { chain ->
                            OShareLanRuntime.ensureSyncedForDeviceQuery()
                            val res = chain.proceed()
                            OShareLanRuntime.syncVirtualDevicesToManager()
                            res
                        }
                }
            }
        }

        for (methodName in listOf("scan", "resume")) {
            runCatching {
                val m = serviceImplCls.getDeclaredMethod(methodName)
                m.isAccessible = true
                module.hook(m)
                    .setId("workspace_oshare_$methodName")
                    .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                    .intercept { chain ->
                        val res = chain.proceed()
                        OShareLanRuntime.syncVirtualDevicesToManager()
                        res
                    }
            }
        }

        if (baseDevCls != null) {
            runCatching {
                val sendData = serviceImplCls.getDeclaredMethod("sendData", Intent::class.java, baseDevCls)
                sendData.isAccessible = true
                module.hook(sendData)
                    .setId("workspace_oshare_send_data")
                    .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                    .intercept { chain ->
                        val intent = chain.getArg(0) as? Intent
                        val target = chain.getArg(1)
                        val mac = runCatching {
                            target?.javaClass?.getMethod("a")?.invoke(target) as? String
                        }.getOrNull()
                        if (intent != null && target != null &&
                            mac?.startsWith(WorkspaceContract.OSHARE_MAC_PREFIX) == true
                        ) {
                            OShareLanRuntime.startLanSend(intent, target)
                            return@intercept null
                        }
                        chain.proceed()
                    }
            }

            runCatching {
                val cancelTask = serviceImplCls.getDeclaredMethod("cancelTask", baseDevCls)
                cancelTask.isAccessible = true
                module.hook(cancelTask)
                    .setId("workspace_oshare_cancel_task")
                    .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                    .intercept { chain ->
                        val target = chain.getArg(0)
                        val mac = runCatching {
                            target?.javaClass?.getMethod("a")?.invoke(target) as? String
                        }.getOrNull()
                        if (target != null && mac?.startsWith(WorkspaceContract.OSHARE_MAC_PREFIX) == true) {
                            OShareLanRuntime.cancelLanSend(target)
                            return@intercept null
                        }
                        chain.proceed()
                    }
            }
        }
    }
}
