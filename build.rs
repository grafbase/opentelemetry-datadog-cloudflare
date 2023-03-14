fn main() {
    // Always rerun if the build script itself changes.
    println!("cargo:rerun-if-changed=build.rs");

    println!("cargo:rerun-if-changed=proto/dd_trace.proto");
    println!("cargo:rerun-if-changed=proto/ddsketch_full.proto");
    println!("cargo:rerun-if-changed=proto/dd_metric.proto");
    println!("cargo:rerun-if-changed=proto/google/pubsub/v1/pubsub.proto");

    let mut prost_build = prost_build::Config::new();
    prost_build.btree_map(["."]);

    tonic_build::configure()
        .compile_with_config(
            prost_build,
            &[
                "proto/ddsketch_full.proto",
                "proto/dd_metric.proto",
                "proto/dd_trace.proto",
                "proto/google/pubsub/v1/pubsub.proto",
            ],
            &["proto/"],
        )
        .unwrap();
}
