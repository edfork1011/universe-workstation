fn main() {
    println!("cargo:rerun-if-changed=src/nautilus_ext.c");

    let lib = pkg_config::Config::new()
        .atleast_version("43.0")
        .cargo_metadata(false)
        .probe("libnautilus-extension-4")
        .expect("libnautilus-extension-4 development headers must be installed");

    let mut build = cc::Build::new();
    build.file("src/nautilus_ext.c").warnings(true);
    for inc in &lib.include_paths {
        build.include(inc);
    }
    build.compile("workspace_nautilus_ext");

    pkg_config::Config::new()
        .atleast_version("43.0")
        .probe("libnautilus-extension-4")
        .expect("libnautilus-extension-4 link failed");
}
