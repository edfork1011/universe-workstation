//! Unified software version constants loaded from root `version.properties`.

/// Canonical package/module version slug (e.g., `1.0-ant`).
pub const VERSION_NAME: &str = env!("WS_VERSION_NAME");

/// Human-readable UI display version (e.g., `1.0 Ant`).
pub const VERSION_DISPLAY: &str = env!("WS_VERSION_DISPLAY");

/// Combined display + slug label (e.g., `1.0 Ant (1.0-ant)`).
pub const VERSION_FULL_LABEL: &str =
    concat!(env!("WS_VERSION_DISPLAY"), " (", env!("WS_VERSION_NAME"), ")");

/// Integer build/version code shared with Android APK, LSPosed, and KernelSU manifests.
pub fn version_code() -> u32 {
    env!("WS_VERSION_CODE")
        .parse()
        .expect("WS_VERSION_CODE must be a valid u32")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{VERSION_DISPLAY, VERSION_FULL_LABEL, VERSION_NAME, version_code};

    fn parse_kv(raw: &str) -> HashMap<&str, &str> {
        let mut map = HashMap::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                map.insert(k.trim(), v.trim());
            }
        }
        map
    }

    #[test]
    fn unified_version_matches_all_module_manifests() {
        let root_props = parse_kv(include_str!("../../../version.properties"));
        let ksu_props = parse_kv(include_str!("../../../android/ksu-module/module.prop"));
        let xposed_props = parse_kv(include_str!(
            "../../../android/app/src/main/resources/META-INF/xposed/module.prop"
        ));

        assert_eq!(VERSION_NAME, root_props["VERSION_NAME"]);
        assert_eq!(VERSION_DISPLAY, root_props["VERSION_DISPLAY"]);
        assert_eq!(VERSION_NAME, "1.0-ant");
        assert_eq!(VERSION_DISPLAY, "1.0 Ant");
        assert_eq!(VERSION_FULL_LABEL, "1.0 Ant (1.0-ant)");
        assert_eq!(
            version_code().to_string(),
            root_props["VERSION_CODE"],
            "VERSION_CODE mismatch"
        );
        assert_eq!(
            env!("CARGO_PKG_VERSION"),
            root_props["CARGO_VERSION"],
            "Cargo.toml version mismatch"
        );

        assert_eq!(ksu_props["version"], VERSION_NAME);
        assert_eq!(ksu_props["versionCode"], root_props["VERSION_CODE"]);

        assert_eq!(xposed_props["version"], VERSION_NAME);
        assert_eq!(xposed_props["versionCode"], root_props["VERSION_CODE"]);
    }
}
