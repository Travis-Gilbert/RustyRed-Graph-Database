// Compile the rustyred.v1 proto into Rust tonic bindings at build time.
//
// Source resolution order:
//   1. `vendor/proto/rustyred/v1/rustyred.proto` — the in-tree vendored
//      snapshot. Always present, kept in sync with the submodule via
//      `scripts/sync-vendored-proto.sh` and CI. This is what hermetic
//      Docker / Railway builds consume.
//   2. `proto/rustyred/v1/rustyred.proto` — the `theorem-protos`
//      submodule. Populated by `git submodule update --init` for
//      developers who edit the upstream contract. Kept as a fallback
//      so a freshly-edited proto can be tested without re-running the
//      sync script.
//
// If neither path exists, the build fails with a message that points
// at the sync script.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");

    let vendored_root = workspace_root.join("vendor").join("proto");
    let submodule_root = workspace_root.join("proto");

    let vendored_proto = vendored_root
        .join("rustyred")
        .join("v1")
        .join("rustyred.proto");
    let submodule_proto = submodule_root
        .join("rustyred")
        .join("v1")
        .join("rustyred.proto");

    // Re-run if either source changes. cargo:rerun-if-changed is additive
    // and tolerates missing paths, so listing both is safe. Emit before
    // the match consumes the PathBufs.
    println!("cargo:rerun-if-changed={}", vendored_proto.display());
    println!("cargo:rerun-if-changed={}", submodule_proto.display());

    let (proto_root, proto_path) = if vendored_proto.exists() {
        (vendored_root, vendored_proto)
    } else if submodule_proto.exists() {
        (submodule_root, submodule_proto)
    } else {
        return Err(format!(
            "Cannot find rustyred.proto. Looked in:\n  {}\n  {}\nRun `scripts/sync-vendored-proto.sh` or `git submodule update --init` before building.",
            vendored_proto.display(),
            submodule_proto.display(),
        )
        .into());
    };

    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&[&proto_path], &[&proto_root])?;

    Ok(())
}
