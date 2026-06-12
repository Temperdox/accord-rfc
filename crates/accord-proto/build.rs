//! Build script for `accord-proto`.
//!
//! Pipeline: `protox` (pure-Rust protobuf compiler) parses the `.proto` files in
//! the workspace `proto/` directory into a `FileDescriptorSet`, which
//! `tonic-build` then turns into client + server Rust code written to `OUT_DIR`.
//!
//! Using `protox` instead of `prost-build`'s default path means we do NOT need a
//! `protoc` binary installed on the machine - the whole toolchain is Cargo
//! crates, which keeps the build reproducible on Windows/macOS/Linux alike.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `CARGO_MANIFEST_DIR` = crates/accord-proto; the shared protos live two
    // levels up under proto/.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let proto_dir = manifest_dir.join("..").join("..").join("proto");

    let proto_files = [
        "common.proto",
        "auth.proto",
        "messaging.proto",
        "mls.proto",
        "groups.proto",
        "backup.proto",
        "roles.proto",
        "vault.proto",
        "friends.proto",
    ];
    let proto_paths: Vec<PathBuf> = proto_files.iter().map(|f| proto_dir.join(f)).collect();

    // Rebuild whenever any .proto changes.
    for path in &proto_paths {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    // Parse to a descriptor set. protox bundles the well-known types
    // (e.g. google/protobuf/timestamp.proto), so the import resolves cleanly.
    let file_descriptor_set = protox::compile(&proto_paths, [&proto_dir])?;

    // Generate both the client stubs and the server traits for every service.
    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_fds(file_descriptor_set)?;

    Ok(())
}
