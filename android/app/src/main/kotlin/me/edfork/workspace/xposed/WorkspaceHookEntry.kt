package me.edfork.workspace.xposed

import android.app.Application
import io.github.libxposed.api.XposedModule
import io.github.libxposed.api.XposedModuleInterface.HotReloadedParam
import io.github.libxposed.api.XposedModuleInterface.HotReloadingParam
import io.github.libxposed.api.XposedModuleInterface.ModuleLoadedParam
import io.github.libxposed.api.XposedModuleInterface.PackageLoadedParam
import me.edfork.workspace.logging.WorkspaceLog
import me.edfork.workspace.model.WorkspaceContract
import me.edfork.workspace.oshare.OShareHookInstaller
import me.edfork.workspace.oshare.OShareLanRuntime
import me.edfork.workspace.systemui.SystemUiHookInstaller
import me.edfork.workspace.systemui.SystemUiSwipeRuntime

/**
 * LSPosed API 102 entrypoint targeting `com.android.systemui` (LAN-gated clipboard gestures)
 * and `com.coloros.oshare` (OPPO Share LAN PC device injection & direct mTLS transfers).
 */
class WorkspaceHookEntry : XposedModule() {

    override fun onModuleLoaded(param: ModuleLoadedParam) {
        WorkspaceLog.attachModule(this)
        WorkspaceLog.info(
            "module_entry",
            "寰宇工作台 loaded in ${param.processName} ($frameworkName $frameworkVersion API $apiVersion)",
        )
    }

    override fun onPackageLoaded(param: PackageLoadedParam) {
        if (!param.isFirstPackage) return
        when (param.packageName) {
            WorkspaceContract.SYSTEM_UI_PACKAGE -> {
                WorkspaceLog.info("module_entry", "Installing SystemUI gated clipboard gesture hooks")
                SystemUiHookInstaller.install(this, param.defaultClassLoader)
            }
            WorkspaceContract.OSHARE_PACKAGE -> {
                WorkspaceLog.info("module_entry", "Installing OShare LAN file transfer hooks")
                OShareHookInstaller.install(this, param.defaultClassLoader)
            }
        }
    }

    override fun onHotReloading(param: HotReloadingParam): Boolean {
        val savedApp = SystemUiSwipeRuntime.shutdownForHotReload()
            ?: OShareLanRuntime.shutdownForHotReload()
            ?: currentApplicationFallback()
        param.setSavedInstanceState(savedApp)
        WorkspaceLog.attachModule(null)
        return true
    }

    override fun onHotReloaded(param: HotReloadedParam) {
        WorkspaceLog.attachModule(this)
        param.oldHookHandles.forEach { it.unhook() }
        val app = (param.savedInstanceState as? Application) ?: currentApplicationFallback()
        val cl = app?.classLoader ?: Thread.currentThread().contextClassLoader ?: return
        when (param.processName) {
            WorkspaceContract.SYSTEM_UI_PACKAGE -> {
                SystemUiHookInstaller.install(this, cl)
                if (app != null) {
                    SystemUiSwipeRuntime.initialize(app)
                }
            }
            WorkspaceContract.OSHARE_PACKAGE -> {
                OShareHookInstaller.install(this, cl)
                if (app != null) {
                    OShareLanRuntime.initialize(app, cl)
                }
            }
        }
    }

    private fun currentApplicationFallback(): Application? =
        runCatching {
            val at = Class.forName("android.app.ActivityThread")
            at.getMethod("currentApplication").invoke(null) as? Application
        }.getOrNull()
}
