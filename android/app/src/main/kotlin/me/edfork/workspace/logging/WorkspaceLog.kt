package me.edfork.workspace.logging

import android.util.Log
import io.github.libxposed.api.XposedModule

object WorkspaceLog {
    private const val TAG = "WorkspaceBridge"

    @Volatile
    private var moduleRef: XposedModule? = null

    fun attachModule(module: XposedModule?) {
        moduleRef = module
    }

    fun info(component: String, message: String) {
        val line = "[$component] $message"
        runCatching { Log.i(TAG, line) }
        runCatching { moduleRef?.log(Log.INFO, TAG, line) }
    }

    fun warn(component: String, message: String, error: Throwable? = null) {
        val line = "[$component] $message"
        runCatching { Log.w(TAG, line, error) }
        runCatching {
            if (error != null) {
                moduleRef?.log(Log.WARN, TAG, line, error)
            } else {
                moduleRef?.log(Log.WARN, TAG, line)
            }
        }
    }

    fun error(component: String, message: String, error: Throwable? = null) {
        val line = "[$component] $message"
        runCatching { Log.e(TAG, line, error) }
        runCatching {
            if (error != null) {
                moduleRef?.log(Log.ERROR, TAG, line, error)
            } else {
                moduleRef?.log(Log.ERROR, TAG, line)
            }
        }
    }
}
