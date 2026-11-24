package me.edfork.workspace.systemui

import android.app.Application
import android.app.Instrumentation
import android.content.Context
import io.github.libxposed.api.XposedInterface
import io.github.libxposed.api.XposedModule
import me.edfork.workspace.logging.WorkspaceLog
import me.edfork.workspace.model.WorkspaceContract

object SystemUiHookInstaller {
    private const val HOOK_INSTR = "workspace_sysui_instrumentation"
    private const val HOOK_APP = "workspace_sysui_app_create"

    fun install(module: XposedModule, classLoader: ClassLoader) {
        runCatching {
            val method = Instrumentation::class.java.getDeclaredMethod(
                "callApplicationOnCreate",
                Application::class.java,
            )
            method.isAccessible = true
            module.hook(method)
                .setId(HOOK_INSTR)
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    val app = chain.getArg(0) as? Application
                    val res = chain.proceed()
                    if (app?.packageName == WorkspaceContract.SYSTEM_UI_PACKAGE) {
                        SystemUiSwipeRuntime.initialize(app)
                    }
                    res
                }
        }.onFailure { err ->
            WorkspaceLog.warn("sysui_hooks", "Instrumentation hook failed", err)
        }

        runCatching {
            val appCls = classLoader.loadClass("com.android.systemui.SystemUIApplication")
            val onCreate = appCls.getDeclaredMethod("onCreate")
            onCreate.isAccessible = true
            module.hook(onCreate)
                .setId(HOOK_APP)
                .setExceptionMode(XposedInterface.ExceptionMode.PROTECTIVE)
                .intercept { chain ->
                    val target = chain.thisObject
                    val res = chain.proceed()
                    val ctx = (target as? Context)?.let { it.applicationContext ?: it }
                    if (ctx != null) {
                        SystemUiSwipeRuntime.initialize(ctx)
                    }
                    res
                }
        }.onFailure { err ->
            WorkspaceLog.warn("sysui_hooks", "SystemUIApplication#onCreate hook skipped", err)
        }
    }
}
