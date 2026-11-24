// SPDX-License-Identifier: MIT
package me.edfork.workspace.bridge;

import android.content.AttributionSource;
import android.content.ClipData;
import android.content.ClipDescription;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.res.AssetFileDescriptor;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.media.MediaScannerConnection;
import android.net.Uri;
import android.os.Binder;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Looper;
import android.os.ParcelFileDescriptor;
import android.os.PersistableBundle;
import android.os.Process;
import android.util.Base64;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.util.Locale;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/**
 * UID-1000 (AID_SYSTEM) / fallback-shell app_process bridge for Workspace (寰宇工作台).
 *
 * <p>Uses {@code ActivityManager.getService().getContentProviderExternal(...)} + explicit
 * {@link AttributionSource} instead of {@code Context.getContentResolver()}, avoiding
 * {@code SecurityException: Unable to find app for caller ...} inside standalone {@code app_process}.
 */
public final class ClipboardBridge {
    private static final String SYSTEM_PKG = "android";
    private static final String SHELL_PKG = "com.android.shell";
    private static final String CLIP_PROVIDER_AUTHORITY = "me.edfork.workspace.clipprovider";
    private static final int USER_ID = 0;
    private static final int MAX_IMAGE_BYTES = 20 * 1024 * 1024;

    private ClipboardBridge() {}

    public static void main(String[] args) {
        if (args == null || args.length == 0) {
            printError("INVALID_MESSAGE", "missing subcommand");
            System.exit(2);
        }

        if (Looper.getMainLooper() == null) {
            Looper.prepareMainLooper();
        }

        String cmd = args[0];
        try {
            switch (cmd) {
                case "check":
                    handleCheck();
                    break;
                case "read":
                    handleRead(args.length > 1 ? args[1] : "/data/local/tmp/workspace");
                    break;
                case "write-text":
                    if (args.length < 2) {
                        printError("INVALID_MESSAGE", "missing base64 text");
                        System.exit(2);
                    }
                    handleWriteText(args[1]);
                    break;
                case "write-image":
                    if (args.length < 3) {
                        printError("INVALID_MESSAGE", "usage: write-image <mime> <file>");
                        System.exit(2);
                    }
                    handleWriteImage(args[1], args[2]);
                    break;
                case "scan-media":
                    if (args.length < 2) {
                        printError("INVALID_MESSAGE", "usage: scan-media <file>");
                        System.exit(2);
                    }
                    handleScanMedia(args[1]);
                    break;
                default:
                    printError("INVALID_MESSAGE", "unknown command: " + cmd);
                    System.exit(2);
            }
        } catch (Throwable t) {
            String msg = rootCauseMessage(t);
            printError("CLIPBOARD_READ_FAILED", msg);
            System.exit(1);
        }
    }

    private static String callerPkg() {
        int uid = Process.myUid();
        if (uid == Process.SYSTEM_UID) {
            return SYSTEM_PKG;
        }
        if (uid == Process.ROOT_UID) {
            return "root";
        }
        return SHELL_PKG;
    }

    private static Context getSystemContext() throws Exception {
        Class<?> atCls = Class.forName("android.app.ActivityThread");
        Method systemMain = atCls.getMethod("systemMain");
        Object at = systemMain.invoke(null);
        Method getCtx = atCls.getMethod("getSystemContext");
        Context ctx = (Context) getCtx.invoke(at);
        if (ctx != null) {
            return ctx;
        }
        throw new IllegalStateException("ActivityThread.getSystemContext() returned null");
    }

    private static Object getIClipboard() throws Exception {
        Class<?> smCls = Class.forName("android.os.ServiceManager");
        IBinder binder = (IBinder) smCls.getMethod("getService", String.class).invoke(null, "clipboard");
        if (binder == null) {
            throw new IllegalStateException("clipboard binder unavailable");
        }
        Class<?> stubCls = Class.forName("android.content.IClipboard$Stub");
        return stubCls.getMethod("asInterface", IBinder.class).invoke(null, binder);
    }

    private static ClipData getPrimaryClipCompat(Context ctx, Object clipboard) throws Exception {
        String pkg = callerPkg();
        Throwable lastError = null;

        for (Method m : clipboard.getClass().getMethods()) {
            if (!"getPrimaryClip".equals(m.getName())) continue;
            Class<?>[] params = m.getParameterTypes();
            try {
                if (params.length == 2 && params[0] == String.class && params[1] == int.class) {
                    return (ClipData) m.invoke(clipboard, pkg, USER_ID);
                }
                if (params.length == 3 && params[0] == String.class && params[1] == String.class && params[2] == int.class) {
                    return (ClipData) m.invoke(clipboard, pkg, null, USER_ID);
                }
                if (params.length == 4 && params[0] == String.class && params[1] == String.class && params[2] == int.class && params[3] == int.class) {
                    return (ClipData) m.invoke(clipboard, pkg, null, USER_ID, 0);
                }
                if (params.length == 5 && params[0] == String.class && params[1] == String.class && params[2] == int.class && params[3] == int.class && params[4] == int.class) {
                    return (ClipData) m.invoke(clipboard, pkg, null, USER_ID, 0, 0);
                }
            } catch (Throwable t) {
                lastError = t;
            }
        }

        try {
            ClipboardManager cm = (ClipboardManager) ctx.getSystemService(Context.CLIPBOARD_SERVICE);
            if (cm != null) {
                return cm.getPrimaryClip();
            }
        } catch (Throwable t) {
            if (lastError == null) {
                lastError = t;
            }
        }

        if (lastError != null) {
            throw new IllegalStateException(
                "getPrimaryClip failed for pkg=" + pkg + " uid=" + Process.myUid() + ": " + rootCauseMessage(lastError)
            );
        }
        throw new NoSuchMethodException("No compatible IClipboard.getPrimaryClip overload");
    }

    private static void setPrimaryClipCompat(Context ctx, Object clipboard, ClipData clip) throws Exception {
        String pkg = callerPkg();
        Throwable lastError = null;

        for (Method m : clipboard.getClass().getMethods()) {
            if (!"setPrimaryClip".equals(m.getName())) continue;
            Class<?>[] params = m.getParameterTypes();
            try {
                if (params.length == 3 && params[0] == ClipData.class && params[1] == String.class && params[2] == int.class) {
                    m.invoke(clipboard, clip, pkg, USER_ID);
                    return;
                }
                if (params.length == 4 && params[0] == ClipData.class && params[1] == String.class && params[2] == String.class && params[3] == int.class) {
                    m.invoke(clipboard, clip, pkg, null, USER_ID);
                    return;
                }
                if (params.length == 5 && params[0] == ClipData.class && params[1] == String.class && params[2] == String.class && params[3] == int.class && params[4] == int.class) {
                    m.invoke(clipboard, clip, pkg, null, USER_ID, 0);
                    return;
                }
                if (params.length == 6 && params[0] == ClipData.class && params[1] == String.class && params[2] == String.class && params[3] == int.class && params[4] == int.class && params[5] == int.class) {
                    m.invoke(clipboard, clip, pkg, null, USER_ID, 0, 0);
                    return;
                }
            } catch (Throwable t) {
                lastError = t;
            }
        }

        try {
            ClipboardManager cm = (ClipboardManager) ctx.getSystemService(Context.CLIPBOARD_SERVICE);
            if (cm != null) {
                cm.setPrimaryClip(clip);
                return;
            }
        } catch (Throwable t) {
            if (lastError == null) {
                lastError = t;
            }
        }

        if (lastError != null) {
            throw new IllegalStateException(
                "setPrimaryClip failed for pkg=" + pkg + " uid=" + Process.myUid() + ": " + rootCauseMessage(lastError)
            );
        }
        throw new NoSuchMethodException("No compatible IClipboard.setPrimaryClip overload");
    }

    private static void handleCheck() throws Exception {
        Context ctx = getSystemContext();
        Object clipboard = getIClipboard();
        getPrimaryClipCompat(ctx, clipboard);
        System.out.println("{\"ok\":true,\"kind\":\"check\",\"uid\":" + Process.myUid() + ",\"pkg\":\"" + callerPkg() + "\"}");
    }

    private static void handleRead(String stagingDir) throws Exception {
        Context ctx = getSystemContext();
        Object clipboard = getIClipboard();
        ClipData clip = getPrimaryClipCompat(ctx, clipboard);
        if (clip == null || clip.getItemCount() == 0) {
            printError("CLIPBOARD_EMPTY", "clipboard is empty");
            return;
        }

        ClipDescription desc = clip.getDescription();
        if (desc != null) {
            PersistableBundle extras = desc.getExtras();
            if (extras != null && extras.getBoolean("android.content.extra.IS_SENSITIVE", false)) {
                printError("CLIPBOARD_EXCLUDED", "clipboard item marked sensitive");
                return;
            }
        }

        ClipData.Item item = clip.getItemAt(0);
        Uri uri = item.getUri();
        if (uri != null) {
            String mime = firstImageMimeFromDescription(desc);
            byte[] imageBytes = null;

            String scheme = uri.getScheme();
            String authority = uri.getAuthority();
            if ("content".equalsIgnoreCase(scheme) && authority != null && !authority.isEmpty()) {
                try (ExternalProviderSession session = ExternalProviderSession.acquire(authority)) {
                    if (mime == null) {
                        mime = normalizeImageMime(session.getType(uri));
                    }
                    try (InputStream in = session.openReadStream(uri)) {
                        imageBytes = readBoundedBytes(in);
                    }
                } catch (IllegalStateException tooLarge) {
                    throw tooLarge;
                } catch (Throwable ignored) {
                    imageBytes = null;
                }
            } else if ("file".equalsIgnoreCase(scheme) && uri.getPath() != null) {
                try (InputStream in = new FileInputStream(uri.getPath())) {
                    imageBytes = readBoundedBytes(in);
                } catch (IllegalStateException tooLarge) {
                    throw tooLarge;
                } catch (Throwable ignored) {
                    imageBytes = null;
                }
            }

            if (imageBytes != null && imageBytes.length > 0) {
                String sniffed = sniffImageMime(imageBytes);
                if ("image/png".equals(sniffed) || "image/jpeg".equals(sniffed) || "image/webp".equals(sniffed)) {
                    mime = sniffed;
                } else {
                    Bitmap bmp = BitmapFactory.decodeByteArray(imageBytes, 0, imageBytes.length);
                    if (bmp != null) {
                        ByteArrayOutputStream pngOut = new ByteArrayOutputStream();
                        if (bmp.compress(Bitmap.CompressFormat.PNG, 100, pngOut)) {
                            imageBytes = pngOut.toByteArray();
                            mime = "image/png";
                        }
                        bmp.recycle();
                    } else {
                        mime = null;
                    }
                }

                if (mime != null) {
                    File dir = new File(stagingDir);
                    if (!dir.exists()) {
                        dir.mkdirs();
                    }
                    File outFile = new File(dir, "clip_read_" + System.currentTimeMillis() + ".bin");
                    try (FileOutputStream fos = new FileOutputStream(outFile)) {
                        fos.write(imageBytes);
                    }
                    System.out.println(
                        "{\"ok\":true,\"kind\":\"image\",\"mime\":\"" + jsonEscape(mime) +
                        "\",\"file\":\"" + jsonEscape(outFile.getAbsolutePath()) + "\"}"
                    );
                    return;
                }
            } else if (mime != null) {
                printError("IMAGE_READ_DENIED", "failed to open clipboard image URI: " + uri);
                return;
            }
        }

        CharSequence text = item.getText();
        if (text == null && item.getHtmlText() != null) {
            text = item.coerceToText(ctx);
        }
        if (text == null || text.length() == 0) {
            printError("CLIPBOARD_EMPTY", "clipboard contains no text or supported image");
            return;
        }
        String b64 = Base64.encodeToString(
            text.toString().getBytes(StandardCharsets.UTF_8),
            Base64.NO_WRAP
        );
        System.out.println("{\"ok\":true,\"kind\":\"text\",\"data_b64\":\"" + b64 + "\"}");
    }

    private static byte[] readBoundedBytes(InputStream in) throws Exception {
        if (in == null) {
            return null;
        }
        ByteArrayOutputStream buf = new ByteArrayOutputStream();
        byte[] tmp = new byte[16384];
        int n;
        int total = 0;
        while ((n = in.read(tmp)) != -1) {
            total += n;
            if (total > MAX_IMAGE_BYTES) {
                throw new IllegalStateException("IMAGE_TOO_LARGE: clipboard image exceeds 20 MiB limit");
            }
            buf.write(tmp, 0, n);
        }
        return buf.toByteArray();
    }

    private static String normalizeImageMime(String rawMime) {
        if (rawMime == null) return null;
        String lower = rawMime.trim().toLowerCase(Locale.ROOT);
        if (lower.startsWith("image/")) {
            return lower;
        }
        return null;
    }

    private static String firstImageMimeFromDescription(ClipDescription desc) {
        if (desc == null) return null;
        int count = desc.getMimeTypeCount();
        for (int i = 0; i < count; i++) {
            String candidate = normalizeImageMime(desc.getMimeType(i));
            if (candidate != null) {
                return candidate;
            }
        }
        return null;
    }

    private static String sniffImageMime(byte[] bytes) {
        if (bytes.length >= 8 &&
            (bytes[0] & 0xFF) == 0x89 && bytes[1] == 'P' && bytes[2] == 'N' && bytes[3] == 'G') {
            return "image/png";
        }
        if (bytes.length >= 3 &&
            (bytes[0] & 0xFF) == 0xFF && (bytes[1] & 0xFF) == 0xD8 && (bytes[2] & 0xFF) == 0xFF) {
            return "image/jpeg";
        }
        if (bytes.length >= 12 &&
            bytes[0] == 'R' && bytes[1] == 'I' && bytes[2] == 'F' && bytes[3] == 'F' &&
            bytes[8] == 'W' && bytes[9] == 'E' && bytes[10] == 'B' && bytes[11] == 'P') {
            return "image/webp";
        }
        if (bytes.length >= 4 &&
            bytes[0] == 'G' && bytes[1] == 'I' && bytes[2] == 'F' && bytes[3] == '8') {
            return "image/gif";
        }
        return null;
    }

    private static void handleWriteText(String b64Utf8) throws Exception {
        byte[] raw = Base64.decode(b64Utf8, Base64.DEFAULT);
        String text = new String(raw, StandardCharsets.UTF_8);
        Context ctx = getSystemContext();
        Object clipboard = getIClipboard();
        ClipData clip = ClipData.newPlainText("寰宇工作台", text);
        setPrimaryClipCompat(ctx, clipboard, clip);
        System.out.println("{\"ok\":true,\"kind\":\"text\",\"size\":" + raw.length + "}");
    }

    private static void handleWriteImage(String mime, String path) throws Exception {
        File file = new File(path);
        if (!file.isFile()) {
            printError("IMAGE_NOT_FOUND", "staged image missing: " + path);
            System.exit(1);
        }

        Uri contentUri;
        String resolvedMime;
        try (ExternalProviderSession session = ExternalProviderSession.acquire(CLIP_PROVIDER_AUTHORITY)) {
            Bundle prepared = session.call("prepare", mime, null);
            if (prepared == null || !prepared.getBoolean("ok", false)) {
                String err = prepared != null ? prepared.getString("error", "prepare returned ok=false") : "prepare returned null";
                printError("CLIPBOARD_WRITE_FAILED", "ClipboardImageProvider.prepare failed: " + err);
                System.exit(1);
                return;
            }

            String uriStr = prepared.getString("uri");
            if (uriStr == null || uriStr.isEmpty()) {
                printError("CLIPBOARD_WRITE_FAILED", "ClipboardImageProvider returned empty URI");
                System.exit(1);
                return;
            }
            resolvedMime = prepared.getString("mime", mime);
            contentUri = Uri.parse(uriStr);

            ParcelFileDescriptor pfd = session.openFile(contentUri, "wt");
            if (pfd == null) {
                printError("CLIPBOARD_WRITE_FAILED", "openFile returned null for " + uriStr);
                System.exit(1);
                return;
            }
            try (InputStream in = new FileInputStream(file);
                 OutputStream out = new ParcelFileDescriptor.AutoCloseOutputStream(pfd)) {
                byte[] buf = new byte[16384];
                int n;
                while ((n = in.read(buf)) != -1) {
                    out.write(buf, 0, n);
                }
                out.flush();
                try {
                    pfd.getFileDescriptor().sync();
                } catch (Throwable ignored) {}
            }
        }

        Context ctx = getSystemContext();
        Object clipboard = getIClipboard();
        ClipDescription desc = new ClipDescription("寰宇工作台 Image", new String[] { resolvedMime });
        ClipData clip = new ClipData(desc, new ClipData.Item(contentUri));
        setPrimaryClipCompat(ctx, clipboard, clip);
        System.out.println("{\"ok\":true,\"kind\":\"image\",\"uri\":\"" + jsonEscape(contentUri.toString()) + "\"}");
    }

    private static void handleScanMedia(String filePath) throws Exception {
        Context ctx = getSystemContext();
        CountDownLatch latch = new CountDownLatch(1);
        MediaScannerConnection.scanFile(
            ctx,
            new String[] { filePath },
            null,
            (path, uri) -> latch.countDown()
        );
        latch.await(3, TimeUnit.SECONDS);
        Intent scanIntent = new Intent(Intent.ACTION_MEDIA_SCANNER_SCAN_FILE);
        scanIntent.setData(Uri.fromFile(new File(filePath)));
        ctx.sendBroadcast(scanIntent);
        System.out.println("{\"ok\":true,\"kind\":\"scan-media\"}");
    }

    /**
     * Accesses exported ContentProviders directly via {@code IActivityManager.getContentProviderExternal}
     * without requiring a registered {@code ApplicationThread}.
     */
    private static final class ExternalProviderSession implements AutoCloseable {
        private final Object iam;
        private final String authority;
        private final IBinder token;
        private final Object provider;

        private ExternalProviderSession(Object iam, String authority, IBinder token, Object provider) {
            this.iam = iam;
            this.authority = authority;
            this.token = token;
            this.provider = provider;
        }

        static ExternalProviderSession acquire(String authority) throws Exception {
            Class<?> amCls = Class.forName("android.app.ActivityManager");
            Object iam = amCls.getMethod("getService").invoke(null);
            if (iam == null) {
                throw new IllegalStateException("ActivityManager.getService() returned null");
            }
            IBinder token = new Binder();
            Object holder = null;
            Throwable lastError = null;

            for (Method m : iam.getClass().getMethods()) {
                if (!"getContentProviderExternal".equals(m.getName())) continue;
                Class<?>[] params = m.getParameterTypes();
                try {
                    if (params.length == 4 && params[0] == String.class && params[1] == int.class && IBinder.class.isAssignableFrom(params[2]) && params[3] == String.class) {
                        holder = m.invoke(iam, authority, USER_ID, token, "*clipbridge*");
                        break;
                    }
                    if (params.length == 3 && params[0] == String.class && params[1] == int.class && IBinder.class.isAssignableFrom(params[2])) {
                        holder = m.invoke(iam, authority, USER_ID, token);
                        break;
                    }
                } catch (Throwable t) {
                    lastError = t;
                }
            }

            if (holder == null) {
                String suffix = lastError != null ? ": " + rootCauseMessage(lastError) : "";
                throw new IllegalStateException("Could not find provider: " + authority + suffix);
            }
            Object provider = holder.getClass().getField("provider").get(holder);
            if (provider == null) {
                throw new IllegalStateException("ContentProviderHolder.provider is null for: " + authority);
            }
            return new ExternalProviderSession(iam, authority, token, provider);
        }

        private static AttributionSource buildAttributionSource() {
            return new AttributionSource.Builder(Process.myUid())
                .setPackageName(callerPkg())
                .build();
        }

        Bundle call(String methodName, String arg, Bundle extras) throws Exception {
            AttributionSource attr = buildAttributionSource();
            for (Method m : provider.getClass().getMethods()) {
                if (!"call".equals(m.getName())) continue;
                Class<?>[] params = m.getParameterTypes();
                if (params.length == 5
                    && params[0] == AttributionSource.class
                    && params[1] == String.class
                    && params[2] == String.class
                    && params[3] == String.class
                    && params[4] == Bundle.class) {
                    Bundle res = (Bundle) m.invoke(provider, attr, authority, methodName, arg, extras);
                    if (res != null) {
                        res.size();
                    }
                    return res;
                }
            }
            for (Method m : provider.getClass().getMethods()) {
                if (!"call".equals(m.getName())) continue;
                Class<?>[] params = m.getParameterTypes();
                if (params.length == 6
                    && params[0] == String.class
                    && params[1] == String.class
                    && params[2] == String.class
                    && params[3] == String.class
                    && params[4] == String.class
                    && params[5] == Bundle.class) {
                    Bundle res = (Bundle) m.invoke(provider, callerPkg(), null, authority, methodName, arg, extras);
                    if (res != null) {
                        res.size();
                    }
                    return res;
                }
            }
            throw new NoSuchMethodException("No compatible IContentProvider.call overload");
        }

        ParcelFileDescriptor openFile(Uri uri, String mode) throws Exception {
            AttributionSource attr = buildAttributionSource();
            for (Method m : provider.getClass().getMethods()) {
                if (!"openFile".equals(m.getName())) continue;
                Class<?>[] params = m.getParameterTypes();
                if (params.length == 4 && params[0] == AttributionSource.class && params[1] == Uri.class && params[2] == String.class) {
                    return (ParcelFileDescriptor) m.invoke(provider, attr, uri, mode, null);
                }
                if (params.length == 5 && params[0] == AttributionSource.class && params[1] == Uri.class && params[2] == String.class) {
                    Object extra = params[4] == int.class ? 0 : null;
                    return (ParcelFileDescriptor) m.invoke(provider, attr, uri, mode, null, extra);
                }
            }
            throw new NoSuchMethodException("No compatible IContentProvider.openFile overload");
        }

        InputStream openReadStream(Uri uri) throws Exception {
            AttributionSource attr = buildAttributionSource();
            for (Method m : provider.getClass().getMethods()) {
                if (!"openAssetFile".equals(m.getName())) continue;
                Class<?>[] params = m.getParameterTypes();
                if (params.length == 4 && params[0] == AttributionSource.class && params[1] == Uri.class && params[2] == String.class) {
                    AssetFileDescriptor afd = (AssetFileDescriptor) m.invoke(provider, attr, uri, "r", null);
                    if (afd != null) {
                        return afd.createInputStream();
                    }
                }
            }
            ParcelFileDescriptor pfd = openFile(uri, "r");
            if (pfd == null) {
                return null;
            }
            return new ParcelFileDescriptor.AutoCloseInputStream(pfd);
        }

        String getType(Uri uri) {
            AttributionSource attr = buildAttributionSource();
            for (Method m : provider.getClass().getMethods()) {
                if (!"getType".equals(m.getName())) continue;
                Class<?>[] params = m.getParameterTypes();
                try {
                    if (params.length == 2 && params[0] == AttributionSource.class && params[1] == Uri.class) {
                        return (String) m.invoke(provider, attr, uri);
                    }
                    if (params.length == 1 && params[0] == Uri.class) {
                        return (String) m.invoke(provider, uri);
                    }
                } catch (Throwable ignored) {}
            }
            return null;
        }

        @Override
        public void close() {
            for (Method m : iam.getClass().getMethods()) {
                if (!"removeContentProviderExternalAsUser".equals(m.getName())) continue;
                Class<?>[] params = m.getParameterTypes();
                if (params.length == 3 && params[0] == String.class && IBinder.class.isAssignableFrom(params[1]) && params[2] == int.class) {
                    try {
                        m.invoke(iam, authority, token, USER_ID);
                        return;
                    } catch (Throwable ignored) {}
                }
            }
            for (Method m : iam.getClass().getMethods()) {
                if (!"removeContentProviderExternal".equals(m.getName())) continue;
                Class<?>[] params = m.getParameterTypes();
                if (params.length == 2 && params[0] == String.class && IBinder.class.isAssignableFrom(params[1])) {
                    try {
                        m.invoke(iam, authority, token);
                        return;
                    } catch (Throwable ignored) {}
                }
            }
        }
    }

    private static String rootCauseMessage(Throwable t) {
        Throwable cur = t;
        while (cur.getCause() != null && cur.getCause() != cur) {
            cur = cur.getCause();
        }
        String msg = cur.getMessage();
        return (msg != null && !msg.isEmpty()) ? msg : cur.getClass().getSimpleName();
    }

    private static void printError(String code, String msg) {
        System.out.println("{\"ok\":false,\"code\":\"" + jsonEscape(code) + "\",\"message\":\"" + jsonEscape(msg) + "\"}");
    }

    private static String jsonEscape(String s) {
        if (s == null) return "";
        StringBuilder sb = new StringBuilder(s.length() + 8);
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '\\': sb.append("\\\\"); break;
                case '"': sb.append("\\\""); break;
                case '\n': sb.append("\\n"); break;
                case '\r': sb.append("\\r"); break;
                case '\t': sb.append("\\t"); break;
                default:
                    if (c < 0x20) {
                        sb.append(String.format("\\u%04x", (int) c));
                    } else {
                        sb.append(c);
                    }
            }
        }
        return sb.toString();
    }
}
