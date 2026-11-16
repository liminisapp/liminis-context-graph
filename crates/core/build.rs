fn main() {
    // Without any `rerun-if-changed`, cargo fingerprints this script by scanning the whole
    // package for its newest file and stores "<mtime> (<path>)". CI sets every tracked file to
    // the commit time, so that newest file is a many-way tie and the recorded path can differ
    // between the build-release and test jobs; the crate then rebuilds ("the precalculated
    // components changed") and trips the FR-006 guard. This script reads nothing but the
    // target family, so only its own source can change its output.
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
        println!("cargo:rustc-link-arg=-rdynamic");
    }
}
