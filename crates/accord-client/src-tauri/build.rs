//! Tauri build script - generates the platform glue (and, on Windows, embeds the
//! app icon into the executable) from `tauri.conf.json`.
//!
//! On Windows it also:
//! * copies the vendored `resources/wintun.dll` next to the built binary
//!   (`tun-rs` loads it from the executable's directory when the mesh creates
//!   its TUN adapter), so a fresh clone's `cargo run --features mesh` works
//!   without a manual download - installers get it via the resources entry in
//!   `tauri.windows.conf.json`;
//! * embeds a `requireAdministrator` manifest into every Windows executable, so
//!   launching the app (installed or double-clicked from target/) triggers UAC
//!   and the mesh TUN adapter can be created. Consequence: `cargo run` from a
//!   NON-elevated terminal fails with ERROR_ELEVATION_REQUIRED (a non-elevated
//!   parent cannot spawn an elevation-required exe) - run cargo from an admin
//!   terminal, or launch the built exe directly. The long-term plan is
//!   on-demand elevation (only when enabling mesh) instead of whole-app
//!   elevation.

use std::path::PathBuf;

/// Windows application manifest requesting elevation. Mirrors Tauri's default
/// manifest (the Common-Controls dependency is required for native dialogs)
/// plus the `requireAdministrator` execution level.
const ELEVATED_MANIFEST: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let attrs = tauri_build::Attributes::new().windows_attributes(
            tauri_build::WindowsAttributes::new().app_manifest(ELEVATED_MANIFEST),
        );
        tauri_build::try_build(attrs).expect("tauri-build failed");
    } else {
        tauri_build::build();
    }
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
