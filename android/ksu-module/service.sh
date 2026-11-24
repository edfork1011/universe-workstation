#!/system/bin/sh
MODDIR=${0%/*}
PKG="me.edfork.workspace"
PIDFILE="/dev/workspaced_service.pid"

if [ -f "$PIDFILE" ]; then
  OLD_PID="$(cat "$PIDFILE" 2>/dev/null)"
  if [ -n "$OLD_PID" ] && [ "$OLD_PID" != "$$" ] && kill -0 "$OLD_PID" 2>/dev/null; then
    exit 0
  fi
fi
echo "$$" > "$PIDFILE"

while [ "$(getprop sys.boot_completed)" != "1" ]; do
  sleep 2
done
while [ ! -d /sdcard/Android ]; do
  sleep 2
done
sleep 2

mkdir -p /data/adb/workspace/state /data/adb/workspace/bin /data/adb/workspace/dex
chmod 0700 /data/adb/workspace/state

mkdir -p /data/local/tmp/workspace
chown 1000:2000 /data/local/tmp/workspace
chmod 0775 /data/local/tmp/workspace

JAR_SRC="/data/adb/workspace/dex/workspace-bridge.jar"
[ -f "$MODDIR/dex/workspace-bridge.jar" ] && JAR_SRC="$MODDIR/dex/workspace-bridge.jar"
if [ -f "$JAR_SRC" ]; then
  rm -f /data/local/tmp/workspace/workspace-bridge.jar
  cp "$JAR_SRC" /data/local/tmp/workspace/workspace-bridge.jar
  chown 1000:2000 /data/local/tmp/workspace/workspace-bridge.jar
  chmod 0644 /data/local/tmp/workspace/workspace-bridge.jar
fi

mkdir -p "/storage/emulated/0/Download/寰宇工作台"

# Exempt clipboard and app background throttling on ColorOS
cmd appops set android READ_CLIPBOARD allow >/dev/null 2>&1 || true
cmd appops set android WRITE_CLIPBOARD allow >/dev/null 2>&1 || true
cmd appops set com.android.shell READ_CLIPBOARD allow >/dev/null 2>&1 || true
cmd appops set com.android.shell WRITE_CLIPBOARD allow >/dev/null 2>&1 || true
cmd appops set "$PKG" READ_CLIPBOARD allow >/dev/null 2>&1 || true
cmd appops set "$PKG" WRITE_CLIPBOARD allow >/dev/null 2>&1 || true
dumpsys deviceidle whitelist +"$PKG" >/dev/null 2>&1 || true
cmd appops set "$PKG" RUN_IN_BACKGROUND allow >/dev/null 2>&1 || true
cmd appops set "$PKG" RUN_ANY_IN_BACKGROUND allow >/dev/null 2>&1 || true
cmd appops set "$PKG" SYSTEM_EXEMPT_FROM_HIBERNATION allow >/dev/null 2>&1 || true
cmd appops set "$PKG" SYSTEM_EXEMPT_FROM_POWER_RESTRICTIONS allow >/dev/null 2>&1 || true
am set-standby-bucket "$PKG" active >/dev/null 2>&1 || true

DAEMON_BIN="/data/adb/workspace/bin/workspaced"
[ -x "$MODDIR/bin/workspaced" ] && DAEMON_BIN="$MODDIR/bin/workspaced"

LOG_FILE="/data/adb/workspace/workspaced.log"
while true; do
  if [ -z "$(pidof workspaced 2>/dev/null)" ] && [ -x "$DAEMON_BIN" ]; then
    "$DAEMON_BIN" --state-dir /data/adb/workspace/state daemon >>"$LOG_FILE" 2>&1 &
  fi
  sleep 5
done
