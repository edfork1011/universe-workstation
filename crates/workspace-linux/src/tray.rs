//! Pure Rust D-Bus `org.kde.StatusNotifierItem` + `com.canonical.dbusmenu` system tray.
//!
//! Uses `zbus` directly (zero `libappindicator` / `gtk3` linkage) so the tray coexists cleanly
//! with GTK4 + Wayland panels (Caelestia, Waybar, KDE, etc.).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tracing::{debug, info, trace, warn};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{Connection, interface};

#[derive(Debug, Clone)]
pub enum TrayAction {
    ShowWindow,
    OpenReceiveDir,
    ToggleSyncPaused,
    Quit,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrayStatus {
    pub connected: bool,
    pub peer_summary: String,
    pub sync_paused: bool,
}

struct SharedTrayState {
    status: RwLock<TrayStatus>,
    revision: RwLock<u32>,
    action_tx: mpsc::UnboundedSender<TrayAction>,
}

struct SniItem {
    shared: Arc<SharedTrayState>,
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl SniItem {
    #[zbus(property)]
    async fn category(&self) -> String {
        "ApplicationStatus".to_string()
    }

    #[zbus(property)]
    async fn id(&self) -> String {
        "me.edfork.workspace".to_string()
    }

    #[zbus(property)]
    async fn title(&self) -> String {
        "寰宇工作台".to_string()
    }

    #[zbus(property)]
    async fn status(&self) -> String {
        "Active".to_string()
    }

    #[zbus(property)]
    async fn window_id(&self) -> u32 {
        0
    }

    #[zbus(property)]
    async fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property)]
    async fn menu(&self) -> zbus::fdo::Result<ObjectPath<'static>> {
        ObjectPath::try_from("/MenuBar")
            .map_err(|e| zbus::fdo::Error::Failed(format!("invalid menu path: {e}")))
    }

    #[zbus(property)]
    async fn icon_name(&self) -> String {
        String::new()
    }

    #[zbus(property)]
    async fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        vec![
            (
                64,
                64,
                include_bytes!("../../../assets/tray_icon_64.argb").to_vec(),
            ),
            (
                32,
                32,
                include_bytes!("../../../assets/tray_icon_32.argb").to_vec(),
            ),
        ]
    }

    #[zbus(property)]
    async fn tool_tip(&self) -> (String, Vec<(i32, i32, Vec<u8>)>, String, String) {
        let st = self
            .shared
            .status
            .read()
            .expect("TrayStatus lock poisoned")
            .clone();
        let desc = if st.connected {
            format!("已连接 · {}", st.peer_summary)
        } else {
            format!("局域网监听中 · {}", st.peer_summary)
        };
        (
            String::new(),
            vec![(
                64,
                64,
                include_bytes!("../../../assets/tray_icon_64.argb").to_vec(),
            )],
            "寰宇工作台".to_string(),
            desc,
        )
    }

    async fn context_menu(&self, _x: i32, _y: i32) {
        let _ = self.shared.action_tx.send(TrayAction::ShowWindow);
    }

    async fn activate(&self, _x: i32, _y: i32) {
        let _ = self.shared.action_tx.send(TrayAction::ShowWindow);
    }

    async fn secondary_activate(&self, _x: i32, _y: i32) {
        let _ = self.shared.action_tx.send(TrayAction::ToggleSyncPaused);
    }

    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;
}

type DbusMenuNode = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

struct DbusMenu {
    shared: Arc<SharedTrayState>,
}

fn owned_str(s: &str) -> OwnedValue {
    Value::from(s)
        .try_to_owned()
        .expect("string converts to OwnedValue")
}

fn owned_bool(b: bool) -> OwnedValue {
    Value::from(b)
        .try_to_owned()
        .expect("bool converts to OwnedValue")
}

fn owned_i32(v: i32) -> OwnedValue {
    Value::from(v)
        .try_to_owned()
        .expect("i32 converts to OwnedValue")
}

fn node_to_owned(node: DbusMenuNode) -> OwnedValue {
    Value::from(node)
        .try_to_owned()
        .expect("DbusMenuNode converts to OwnedValue")
}

impl DbusMenu {
    fn build_items(&self) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        let st = self
            .shared
            .status
            .read()
            .expect("TrayStatus lock poisoned")
            .clone();

        let status_label = if st.connected {
            format!("● 已连接：{}", st.peer_summary)
        } else {
            format!("○ 局域网等待连接 ({})", st.peer_summary)
        };

        let mut item1 = HashMap::new();
        item1.insert("label".to_string(), owned_str("打开 寰宇工作台"));
        item1.insert("enabled".to_string(), owned_bool(true));
        item1.insert("visible".to_string(), owned_bool(true));

        let mut item2 = HashMap::new();
        item2.insert("label".to_string(), owned_str(&status_label));
        item2.insert("enabled".to_string(), owned_bool(false));
        item2.insert("visible".to_string(), owned_bool(true));

        let mut item3 = HashMap::new();
        item3.insert("label".to_string(), owned_str("打开接收目录"));
        item3.insert("enabled".to_string(), owned_bool(true));
        item3.insert("visible".to_string(), owned_bool(true));

        let mut item4 = HashMap::new();
        item4.insert("label".to_string(), owned_str("暂停剪贴板共享"));
        item4.insert("toggle-type".to_string(), owned_str("checkmark"));
        item4.insert(
            "toggle-state".to_string(),
            owned_i32(if st.sync_paused { 1 } else { 0 }),
        );
        item4.insert("enabled".to_string(), owned_bool(true));
        item4.insert("visible".to_string(), owned_bool(true));

        let mut sep = HashMap::new();
        sep.insert("type".to_string(), owned_str("separator"));
        sep.insert("visible".to_string(), owned_bool(true));

        let mut item6 = HashMap::new();
        item6.insert("label".to_string(), owned_str("退出"));
        item6.insert("enabled".to_string(), owned_bool(true));
        item6.insert("visible".to_string(), owned_bool(true));

        vec![
            (1, item1),
            (2, item2),
            (3, item3),
            (4, item4),
            (5, sep),
            (6, item6),
        ]
    }
}

#[interface(name = "com.canonical.dbusmenu")]
impl DbusMenu {
    #[zbus(property)]
    async fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    async fn text_direction(&self) -> String {
        "ltr".to_string()
    }

    #[zbus(property)]
    async fn status(&self) -> String {
        "normal".to_string()
    }

    #[zbus(property)]
    async fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    async fn get_layout(
        &self,
        parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, DbusMenuNode)> {
        let rev = *self
            .shared
            .revision
            .read()
            .expect("Tray revision lock poisoned");
        let items = self.build_items();
        if parent_id == 0 {
            let mut root_props = HashMap::new();
            root_props.insert("children-display".to_string(), owned_str("submenu"));
            let children: Vec<OwnedValue> = items
                .into_iter()
                .map(|(id, props)| node_to_owned((id, props, Vec::new())))
                .collect();
            return Ok((rev, (0, root_props, children)));
        }
        if let Some((id, props)) = items.into_iter().find(|(id, _)| *id == parent_id) {
            return Ok((rev, (id, props, Vec::new())));
        }
        Ok((rev, (parent_id, HashMap::new(), Vec::new())))
    }

    async fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<Vec<(i32, HashMap<String, OwnedValue>)>> {
        let items = self.build_items();
        if ids.is_empty() {
            return Ok(items);
        }
        Ok(items
            .into_iter()
            .filter(|(id, _)| ids.contains(id))
            .collect())
    }

    async fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        let items = self.build_items();
        let Some((_, props)) = items.into_iter().find(|(item_id, _)| *item_id == id) else {
            return Err(zbus::fdo::Error::Failed("unknown menu item id".into()));
        };
        props
            .get(&name)
            .cloned()
            .ok_or_else(|| zbus::fdo::Error::Failed("unknown property".into()))
    }

    async fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        if event_id != "clicked" {
            return;
        }
        match id {
            1 => {
                let _ = self.shared.action_tx.send(TrayAction::ShowWindow);
            }
            3 => {
                let _ = self.shared.action_tx.send(TrayAction::OpenReceiveDir);
            }
            4 => {
                let _ = self.shared.action_tx.send(TrayAction::ToggleSyncPaused);
            }
            6 => {
                let _ = self.shared.action_tx.send(TrayAction::Quit);
            }
            _ => {}
        }
    }

    async fn event_group(
        &self,
        events: Vec<(i32, String, OwnedValue, u32)>,
    ) -> zbus::fdo::Result<Vec<i32>> {
        for (id, ev, _, _) in events {
            if ev == "clicked" {
                match id {
                    1 => {
                        let _ = self.shared.action_tx.send(TrayAction::ShowWindow);
                    }
                    3 => {
                        let _ = self.shared.action_tx.send(TrayAction::OpenReceiveDir);
                    }
                    4 => {
                        let _ = self.shared.action_tx.send(TrayAction::ToggleSyncPaused);
                    }
                    6 => {
                        let _ = self.shared.action_tx.send(TrayAction::Quit);
                    }
                    _ => {}
                }
            }
        }
        Ok(Vec::new())
    }

    async fn about_to_show(&self, _id: i32) -> zbus::fdo::Result<bool> {
        Ok(false)
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

/// Start the pure Rust D-Bus SNI system tray service and return its controller handle.
pub fn spawn_tray_service(
    action_tx: mpsc::UnboundedSender<TrayAction>,
    mut status_rx: watch::Receiver<TrayStatus>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let shared = Arc::new(SharedTrayState {
            status: RwLock::new(status_rx.borrow().clone()),
            revision: RwLock::new(1),
            action_tx,
        });

        let bus_name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        let conn = match zbus::connection::Builder::session() {
            Ok(b) => b
                .name(bus_name.as_str())
                .ok()
                .map(|b| b.serve_at("/StatusNotifierItem", SniItem { shared: shared.clone() }))
                .and_then(|res| res.ok())
                .map(|b| b.serve_at("/MenuBar", DbusMenu { shared: shared.clone() }))
                .and_then(|res| res.ok()),
            Err(err) => {
                warn!("failed to create session D-Bus builder: {err}");
                None
            }
        };

        let conn = match conn {
            Some(builder) => match builder.build().await {
                Ok(c) => c,
                Err(err) => {
                    warn!("system tray disabled (D-Bus session unavailable): {err}");
                    return;
                }
            },
            None => {
                warn!("system tray disabled (could not register D-Bus objects)");
                return;
            }
        };

        register_with_watcher(&conn, &bus_name, true).await;
        let mut reregister_tick = tokio::time::interval(Duration::from_secs(12));
        reregister_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = reregister_tick.tick() => {
                    // Re-register gently in case Caelestia / panel host restarted.
                    register_with_watcher(&conn, &bus_name, false).await;
                }
                changed = status_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let next = status_rx.borrow().clone();
                    let changed_flag = {
                        let mut st = shared.status.write().expect("TrayStatus lock poisoned");
                        if *st != next {
                            *st = next;
                            true
                        } else {
                            false
                        }
                    };
                    if changed_flag {
                        let rev = {
                            let mut rev_guard = shared.revision.write().expect("Tray revision lock poisoned");
                            *rev_guard = rev_guard.wrapping_add(1);
                            *rev_guard
                        };
                        emit_tray_updates(&conn, rev).await;
                    }
                }
            }
        }
    })
}

async fn register_with_watcher(conn: &Connection, bus_name: &str, initial: bool) {
    match conn
        .call_method(
            Some("org.kde.StatusNotifierWatcher"),
            "/StatusNotifierWatcher",
            Some("org.kde.StatusNotifierWatcher"),
            "RegisterStatusNotifierItem",
            &(bus_name,),
        )
        .await
    {
        Ok(_) => {
            if initial {
                info!("registered SNI tray icon `{bus_name}`");
            } else {
                trace!("refreshed SNI tray icon registration `{bus_name}`");
            }
        }
        Err(err) => debug!("StatusNotifierWatcher registration skipped: {err}"),
    }
}

async fn emit_tray_updates(conn: &Connection, rev: u32) {
    if let Ok(sni_ref) = conn
        .object_server()
        .interface::<_, SniItem>("/StatusNotifierItem")
        .await
    {
        let emitter = sni_ref.signal_emitter();
        let _ = SniItem::new_icon(emitter).await;
        let _ = SniItem::new_tool_tip(emitter).await;
    }
    if let Ok(menu_ref) = conn
        .object_server()
        .interface::<_, DbusMenu>("/MenuBar")
        .await
    {
        let _ = DbusMenu::layout_updated(menu_ref.signal_emitter(), rev, 0).await;
    }
}
