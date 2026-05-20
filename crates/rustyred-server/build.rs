// Compile the rustyred.v1 proto from the theorem-protos submodule
// into Rust bindings at build time. The submodule lives at the repo
// root under `proto/`. See README.md for setup notes about pulling
// the submodule before first build.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The submodule path is repo-root-relative; `CARGO_MANIFEST_DIR`
    // points at the crate dir, so we go up two levels to reach the
    // workspace root, then into `proto/`.
    let proto_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("proto");

    let rustyred_proto = proto_root.join("rustyred").join("v1").join("rustyred.proto");

    if !rustyred_proto.exists() {
        return Err(format!(
            "Cannot find theorem-protos at {}. Run `git submodule update --init` before building.",
            rustyred_proto.display()
        )
        .into());
    }

    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&[&rustyred_proto], &[&proto_root])?;

    println!("cargo:rerun-if-changed={}", rustyred_proto.display());

    Ok(())
}
