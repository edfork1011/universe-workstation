fn main() {
    println!("cargo:rerun-if-changed=src/adw_ui.c");

    let gtk4 = pkg_config::Config::new()
        .atleast_version("4.10")
        .cargo_metadata(false)
        .probe("gtk4")
        .expect("gtk4 >= 4.10 required");
    let adw = pkg_config::Config::new()
        .atleast_version("1.4")
        .cargo_metadata(false)
        .probe("libadwaita-1")
        .expect("libadwaita-1 >= 1.4 required");

    let mut build = cc::Build::new();
    build.file("src/adw_ui.c").warnings(true);
    for inc in gtk4.include_paths.iter().chain(adw.include_paths.iter()) {
        build.include(inc);
    }
    build.compile("workspace_share_adw");

    pkg_config::Config::new()
        .atleast_version("1.4")
        .probe("libadwaita-1")
        .expect("libadwaita-1 link failed");
    pkg_config::Config::new()
        .atleast_version("4.10")
        .probe("gtk4")
        .expect("gtk4 link failed");
}
