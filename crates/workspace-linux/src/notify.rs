//! Freedesktop desktop notifications over pure Rust D-Bus (`org.freedesktop.Notifications`).

use std::collections::HashMap;

use tracing::debug;
use zbus::zvariant::Value;

const APP_NAME: &str = "寰宇工作台";
const APP_ICON: &str = "me.edfork.workspace";

/// Send a non-blocking freedesktop notification via D-Bus. Errors are silently logged.
pub fn send_notification(summary: impl Into<String>, body: impl Into<String>) {
    let summary = summary.into();
    let body = body.into();
    tokio::spawn(async move {
        if let Err(err) = notify_dbus(&summary, &body).await {
            debug!("desktop notification skipped: {err}");
        }
    });
}

async fn notify_dbus(summary: &str, body: &str) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let hints: HashMap<&str, Value<'_>> = HashMap::new();
    let actions: Vec<&str> = Vec::new();
    let _: u32 = conn
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                APP_NAME,
                0u32,
                APP_ICON,
                summary,
                body,
                actions,
                hints,
                4000i32,
            ),
        )
        .await?
        .body()
        .deserialize()?;
    Ok(())
}
