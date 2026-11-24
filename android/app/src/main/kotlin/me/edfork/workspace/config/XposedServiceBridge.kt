package me.edfork.workspace.config

import io.github.libxposed.service.XposedService
import io.github.libxposed.service.XposedServiceHelper
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import me.edfork.workspace.logging.WorkspaceLog

/**
 * Binds `libxposed-service` at Application startup so Vector / LSPosed status and target scopes
 * are reported accurately in UI.
 */
object XposedServiceBridge {
    sealed interface FrameworkState {
        data object NotConnected : FrameworkState
        data class Connected(
            val frameworkName: String,
            val frameworkVersion: String,
            val apiVersion: Int,
            val scope: List<String>,
        ) : FrameworkState
    }

    private val initialized = AtomicBoolean(false)

    @Volatile
    private var activeService: XposedService? = null

    private val stateMutable = MutableStateFlow<FrameworkState>(FrameworkState.NotConnected)
    val stateFlow: StateFlow<FrameworkState> = stateMutable.asStateFlow()

    fun initialize() {
        if (!initialized.compareAndSet(false, true)) return

        XposedServiceHelper.registerListener(object : XposedServiceHelper.OnServiceListener {
            override fun onServiceBind(service: XposedService) {
                activeService = service
                WorkspaceLog.info(
                    "xposed_service",
                    "Bound to ${service.frameworkName} ${service.frameworkVersion} (API ${service.apiVersion})",
                )
                refreshStatus()
            }

            override fun onServiceDied(service: XposedService) {
                if (activeService === service) {
                    activeService = null
                }
                stateMutable.value = FrameworkState.NotConnected
            }
        })
    }

    fun refreshStatus() {
        val service = activeService
        if (service == null) {
            stateMutable.value = FrameworkState.NotConnected
            return
        }
        runCatching {
            stateMutable.value = FrameworkState.Connected(
                frameworkName = service.frameworkName,
                frameworkVersion = service.frameworkVersion,
                apiVersion = service.apiVersion,
                scope = runCatching { service.scope }.getOrDefault(emptyList()),
            )
        }.onFailure {
            stateMutable.value = FrameworkState.NotConnected
        }
    }
}
