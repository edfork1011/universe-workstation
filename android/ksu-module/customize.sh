#!/system/bin/sh
ui_print "- Installing 寰宇工作台 (me_edfork_workspace) KernelSU module"

mkdir -p /data/adb/workspace/state /data/adb/workspace/bin /data/adb/workspace/dex
chmod 0700 /data/adb/workspace/state
chmod 0755 /data/adb/workspace/bin /data/adb/workspace/dex

mkdir -p /data/local/tmp/workspace
chown 1000:2000 /data/local/tmp/workspace
chmod 0775 /data/local/tmp/workspace

set_perm_recursive "$MODPATH" 0 0 0755 0644
set_perm "$MODPATH/bin/workspaced" 0 0 0755
set_perm "$MODPATH/dex/workspace-bridge.jar" 1000 2000 0644
set_perm "$MODPATH/service.sh" 0 0 0755

rm -f /data/adb/workspace/bin/workspaced
cp "$MODPATH/bin/workspaced" /data/adb/workspace/bin/workspaced
chmod 0755 /data/adb/workspace/bin/workspaced

rm -f /data/adb/workspace/dex/workspace-bridge.jar
cp "$MODPATH/dex/workspace-bridge.jar" /data/adb/workspace/dex/workspace-bridge.jar
chmod 0644 /data/adb/workspace/dex/workspace-bridge.jar

rm -f /data/local/tmp/workspace/workspace-bridge.jar
cp "$MODPATH/dex/workspace-bridge.jar" /data/local/tmp/workspace/workspace-bridge.jar
chown 1000:2000 /data/local/tmp/workspace/workspace-bridge.jar
chmod 0644 /data/local/tmp/workspace/workspace-bridge.jar

cmd appops set android READ_CLIPBOARD allow >/dev/null 2>&1 || true
cmd appops set android WRITE_CLIPBOARD allow >/dev/null 2>&1 || true
cmd appops set com.android.shell READ_CLIPBOARD allow >/dev/null 2>&1 || true
cmd appops set com.android.shell WRITE_CLIPBOARD allow >/dev/null 2>&1 || true
cmd appops set me.edfork.workspace READ_CLIPBOARD allow >/dev/null 2>&1 || true
cmd appops set me.edfork.workspace WRITE_CLIPBOARD allow >/dev/null 2>&1 || true

# Start workspaced immediately during KernelSU Manager flash so it works even before reboot
if [ "${BOOTMODE:-false}" = "true" ] && [ -x /data/adb/workspace/bin/workspaced ]; then
  ui_print "- Starting workspaced daemon immediately..."
  killall workspaced 2>/dev/null || pkill -f '/workspaced' 2>/dev/null || true
  sleep 1
  nohup /data/adb/workspace/bin/workspaced --state-dir /data/adb/workspace/state daemon \
    >>/data/adb/workspace/workspaced.log 2>&1 </dev/null &
fi

ui_print "- Installation complete."
