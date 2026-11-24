package me.edfork.workspace.liveupdate

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.net.Uri
import android.os.Binder
import android.os.Bundle
import android.os.Process
import me.edfork.workspace.logging.WorkspaceLog
import me.edfork.workspace.model.WorkspaceContract

/**
 * Secured ContentProvider endpoint allowing hooked SystemUI & OShare processes to publish
 * Fluid Cloud Live Update notifications through `me.edfork.workspace` cleanly via Binder IPC.
 */
class FluidCloudProvider : ContentProvider() {

    override fun onCreate(): Boolean = true

    override fun call(method: String, arg: String?, extras: Bundle?): Bundle {
        enforceAllowedCaller()
        val ctx = context ?: return Bundle().apply { putBoolean("ok", false) }

        return when (method) {
            METHOD_POST -> {
                val payload = FluidCloudPayload.fromBundle(extras)
                if (payload == null) {
                    WorkspaceLog.warn("fluid_provider", "Ignored invalid FluidCloudPayload bundle")
                    Bundle().apply { putBoolean("ok", false) }
                } else {
                    FluidCloudController.postOrUpdate(ctx, payload)
                    Bundle().apply { putBoolean("ok", true) }
                }
            }
            METHOD_DISMISS -> {
                val slot = FluidSlot.fromName(extras?.getString(FluidCloudPayload.KEY_SLOT) ?: arg)
                val expectedToken = extras?.getLong(FluidCloudPayload.KEY_TOKEN, 0L) ?: 0L
                FluidCloudController.dismiss(ctx, slot, expectedToken)
                Bundle().apply { putBoolean("ok", true) }
            }
            METHOD_STATUS -> Bundle().apply {
                putBoolean("ok", true)
                putBoolean("notifications_enabled", FluidCloudController.areNotificationsEnabled(ctx))
                putBoolean("can_post_promoted", FluidCloudController.canPostPromoted(ctx))
            }
            else -> Bundle().apply {
                putBoolean("ok", false)
                putString("error", "Unknown method: $method")
            }
        }
    }

    private fun enforceAllowedCaller() {
        val uid = Binder.getCallingUid()
        if (uid == Process.myUid() || uid == 0 || uid == Process.SYSTEM_UID || uid == 2000) {
            return
        }
        val callerPkg = callingPackage
        if (callerPkg != null && ALLOWED_CALLERS.contains(callerPkg)) {
            return
        }
        val resolvedPkg = callerPkg ?: context?.packageManager?.getPackagesForUid(uid)?.firstOrNull()
        if (resolvedPkg != null && ALLOWED_CALLERS.contains(resolvedPkg)) {
            return
        }
        throw SecurityException("Unauthorized FluidCloud caller: pkg=$callerPkg uid=$uid")
    }

    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): Cursor? = null

    override fun getType(uri: Uri): String? = null

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = 0

    override fun update(
        uri: Uri,
        values: ContentValues?,
        selection: String?,
        selectionArgs: Array<out String>?,
    ): Int = 0

    companion object {
        const val AUTHORITY = "me.edfork.workspace.fluidcloud"
        val CONTENT_URI: Uri = Uri.parse("content://$AUTHORITY")
        const val METHOD_POST = "post"
        const val METHOD_DISMISS = "dismiss"
        const val METHOD_STATUS = "status"

        private val ALLOWED_CALLERS = setOf(
            "me.edfork.workspace",
            WorkspaceContract.SYSTEM_UI_PACKAGE,
            WorkspaceContract.OSHARE_PACKAGE,
        )
    }
}
