// SPDX-License-Identifier: GPL-3.0-or-later
//! Nautilus 4 right-click extension (`libnautilus_workspace.so`) adding `"Share with Workspace"`.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::path::PathBuf;
use std::process::Command;

unsafe extern "C" {
    fn ws_nautilus_init(module: *mut c_void);
    fn ws_nautilus_shutdown();
    fn ws_nautilus_list_types(types: *mut *const usize, num_types: *mut c_int);
}

/// Initialize Nautilus extension module.
///
/// # Safety
/// `module` must be a valid `GTypeModule*` supplied by Nautilus during module loading.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nautilus_module_initialize(module: *mut c_void) {
    unsafe { ws_nautilus_init(module) };
}

/// Tear down Nautilus extension module.
#[unsafe(no_mangle)]
pub extern "C" fn nautilus_module_shutdown() {
    unsafe { ws_nautilus_shutdown() };
}

/// Export GType list implemented by this Nautilus extension.
///
/// # Safety
/// `types` and `num_types` must be valid non-null out-pointers provided by Nautilus.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nautilus_module_list_types(types: *mut *const usize, num_types: *mut c_int) {
    unsafe { ws_nautilus_list_types(types, num_types) };
}

fn locate_share_binary() -> PathBuf {
    if let Ok(custom) = std::env::var("WORKSPACE_SHARE_BIN") {
        let p = PathBuf::from(custom);
        if p.is_file() {
            return p;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let user_bin = PathBuf::from(home).join(".local/bin/workspace-share");
        if user_bin.is_file() {
            return user_bin;
        }
    }
    for sys in ["/usr/local/bin/workspace-share", "/usr/bin/workspace-share"] {
        let p = PathBuf::from(sys);
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("workspace-share")
}

/// Spawn `workspace-share` for selected regular file paths.
///
/// # Safety
/// `paths` must point to an array of at least `count` valid NUL-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn workspace_nautilus_on_activate(paths: *const *const c_char, count: c_int) {
    if paths.is_null() || count <= 0 {
        return;
    }

    let mut args = Vec::with_capacity(count as usize);
    for idx in 0..count {
        let ptr = unsafe { *paths.offset(idx as isize) };
        if ptr.is_null() {
            continue;
        }
        if let Ok(s) = unsafe { CStr::from_ptr(ptr) }.to_str() {
            args.push(s.to_string());
        }
    }
    if args.is_empty() {
        return;
    }

    let bin = locate_share_binary();
    if let Ok(mut child) = Command::new(bin).args(&args).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}
