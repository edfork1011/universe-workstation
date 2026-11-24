use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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

fn c_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn main() {
    let version_path = Path::new("../../version.properties");
    println!("cargo:rerun-if-changed={}", version_path.display());
    println!("cargo:rerun-if-changed=src/glass_ui.c");
    println!("cargo:rerun-if-changed=../../assets/logo_128.png");
    println!("cargo:rerun-if-changed=../../assets/me.edfork.workspace.png");
    println!("cargo:rerun-if-changed=../../assets/tray_icon_64.argb");
    println!("cargo:rerun-if-changed=../../assets/tray_icon_32.argb");

    let props = parse_properties(version_path);
    let version_name = props
        .get("VERSION_NAME")
        .expect("VERSION_NAME missing in version.properties");
    let version_display = props
        .get("VERSION_DISPLAY")
        .expect("VERSION_DISPLAY missing in version.properties");
    let version_code: u32 = props
        .get("VERSION_CODE")
        .expect("VERSION_CODE missing in version.properties")
        .parse()
        .expect("VERSION_CODE must be a valid u32");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let header = format!(
        "#ifndef WS_VERSION_H\n\
         #define WS_VERSION_H\n\
         #define WS_VERSION_NAME \"{}\"\n\
         #define WS_VERSION_DISPLAY \"{}\"\n\
         #define WS_VERSION_CODE {}\n\
         #endif\n",
        c_escape(version_name),
        c_escape(version_display),
        version_code
    );
    fs::write(out_dir.join("ws_version.h"), header)
        .expect("failed to write ws_version.h");

    let gtk_inc = pkg_config::Config::new()
        .atleast_version("4.14")
        .cargo_metadata(false)
        .probe("gtk4")
        .expect("gtk4 >= 4.14 development libraries must be installed");

    let mut build = cc::Build::new();
    build.file("src/glass_ui.c").warnings(true);
    build.include(&out_dir);
    for inc in &gtk_inc.include_paths {
        build.include(inc);
    }
    build.compile("workspace_glass_ui");

    // Emit shared library link flags AFTER the static archive for GNU ld.
    pkg_config::Config::new()
        .atleast_version("4.14")
        .probe("gtk4")
        .expect("gtk4 >= 4.14 pkg-config probe failed");
}
