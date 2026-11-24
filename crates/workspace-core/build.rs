use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn parse_properties(path: &Path) -> HashMap<String, String> {
    let raw = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let mut map = HashMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

fn main() {
    let version_path = Path::new("../../version.properties");
    println!("cargo:rerun-if-changed={}", version_path.display());

    let props = parse_properties(version_path);
    let version_name = props
        .get("VERSION_NAME")
        .expect("VERSION_NAME missing in version.properties");
    let version_display = props
        .get("VERSION_DISPLAY")
        .expect("VERSION_DISPLAY missing in version.properties");
    let version_code_str = props
        .get("VERSION_CODE")
        .expect("VERSION_CODE missing in version.properties");
    let cargo_version = props
        .get("CARGO_VERSION")
        .expect("CARGO_VERSION missing in version.properties");

    let _: u32 = version_code_str
        .parse()
        .expect("VERSION_CODE in version.properties must be a valid u32");

    let pkg_version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION not set");
    assert_eq!(
        pkg_version, *cargo_version,
        "Cargo.toml workspace.package.version ({pkg_version}) != version.properties CARGO_VERSION ({cargo_version}); run ./scripts/sync-version.sh"
    );

    println!("cargo:rustc-env=WS_VERSION_NAME={version_name}");
    println!("cargo:rustc-env=WS_VERSION_DISPLAY={version_display}");
    println!("cargo:rustc-env=WS_VERSION_CODE={version_code_str}");
}
