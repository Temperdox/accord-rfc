//! Tauri build script - generates the platform glue (and, on Windows, embeds the
//! app icon into the executable) from `tauri.conf.json`.
//!
//! On Windows it also copies the vendored `resources/wintun.dll` next to the
//! built binary: `tun-rs` loads it from the executable's directory when the
//! mesh creates its TUN adapter, so a fresh clone's `cargo run --features mesh`
//! works without a manual download. (Installers get it via the resources entry
//! in `tauri.windows.conf.json`.)

use std::path::PathBuf;

fn main() {
    tauri_build::build();
    copy_wintun_next_to_binary();
}

fn copy_wintun_next_to_binary() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let source = PathBuf::from(manifest_dir)
        .join("resources")
        .join("wintun.dll");
    if !source.exists() {
        println!("cargo:warning=resources/wintun.dll missing; mesh TUN will not work");
        return;
    }
    // OUT_DIR = <target>/<profile>/build/<pkg>-<hash>/out -> the profile dir is
    // three levels up. Best-effort: a failed copy only affects dev mesh runs.
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    let Some(profile_dir) = PathBuf::from(&out_dir)
        .ancestors()
        .nth(3)
        .map(std::path::Path::to_path_buf)
    else {
        return;
    };
    let dest = profile_dir.join("wintun.dll");
    if !dest.exists() {
        if let Err(e) = std::fs::copy(&source, &dest) {
            println!("cargo:warning=could not copy wintun.dll to target dir: {e}");
        }
    }
    println!("cargo:rerun-if-changed={}", source.display());
}
