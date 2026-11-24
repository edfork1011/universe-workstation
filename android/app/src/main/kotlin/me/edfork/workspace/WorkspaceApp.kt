package me.edfork.workspace

import android.app.Application
import me.edfork.workspace.config.XposedServiceBridge

class WorkspaceApp : Application() {
    override fun onCreate() {
        super.onCreate()
        XposedServiceBridge.initialize()
    }
}
