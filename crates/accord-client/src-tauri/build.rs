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
    // Dev/test escape hatch: when `ACCORD_NO_ELEVATION_MANIFEST` is set, skip the
    // `requireAdministrator` manifest so the crate's TEST binaries (which inherit
    // the same manifest and otherwise fail with ERROR_ELEVATION_REQUIRED) run from
    // a normal, non-elevated shell. The client's non-mesh tests (`taverns_it`,
    // `peers`) need no privileges — only the manifest forced elevation. It is
    // UNSET by default, so normal/release builds are unaffected; NEVER set it for
    // a real build (mesh TUN creation needs the elevated manifest).
    println!("cargo:rerun-if-env-changed=ACCORD_NO_ELEVATION_MANIFEST");
    let skip_elevation = std::env::var_os("ACCORD_NO_ELEVATION_MANIFEST").is_some();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && !skip_elevation {
        let attrs = tauri_build::Attributes::new().windows_attributes(
            tauri_build::WindowsAttributes::new().app_manifest(ELEVATED_MANIFEST),
        );
        tauri_build::try_build(attrs).expect("tauri-build failed");
    } else {
        // Non-Windows, or the elevation opt-out: Tauri's default manifest
        // (asInvoker), which launches without UAC.
        tauri_build::build();
    }
    copy_wintun_next_to_binary();
    generate_ringtone_registry();
}

/// Compile the contents of `assets/ringtones/` into the binary.
///
/// Ringtones are `include_bytes!`d rather than shipped as bundle resources so
/// there is no runtime path to resolve - the same code works from `cargo run`
/// and from an installed app, on every platform. Adding a ringtone is therefore
/// "drop a file in that folder and rebuild"; no code or config change.
fn generate_ringtone_registry() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let dir = PathBuf::from(&manifest_dir)
        .join("assets")
        .join("ringtones");
    println!("cargo:rerun-if-changed={}", dir.display());

    // Formats the decoder (symphonia, see voice_engine/ringtone.rs) handles.
    const AUDIO_EXTENSIONS: [&str; 7] = ["mp3", "wav", "flac", "ogg", "oga", "m4a", "mp4"];

    let mut entries: Vec<(String, String, PathBuf)> = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(&dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()) {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // The id is the file stem, so a user's saved choice survives as long
            // as the file keeps its name.
            entries.push((stem.to_owned(), title_case(stem), path));
        }
    }
    // Stable order regardless of how the filesystem lists the directory.
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut code = String::from(
        "// @generated by build.rs from assets/ringtones/ - do not edit.\n\
         pub static BUNDLED: &[BundledRingtone] = &[\n",
    );
    for (id, name, path) in &entries {
        // Paths are absolute, so include_bytes! resolves regardless of the
        // OUT_DIR the generated file is included from.
        code.push_str(&format!(
            "    BundledRingtone {{ id: {:?}, name: {:?}, bytes: include_bytes!({:?}) }},\n",
            id,
            name,
            path.display().to_string()
        ));
        println!("cargo:rerun-if-changed={}", path.display());
    }
    code.push_str("];\n");

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("ringtones.rs");
    std::fs::write(&out, code).expect("write ringtone registry");
}

/// "soft-chime" / "soft_chime" -> "Soft chime", so a filename is a usable label.
fn title_case(stem: &str) -> String {
    let spaced = stem.replace(['-', '_'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
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
