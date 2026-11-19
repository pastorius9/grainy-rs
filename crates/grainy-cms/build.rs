//! Builds the vendored, unmodified LittleCMS 2.19 as a static library, with the flags the Python Grainy uses
//! for its `luma_lcms2` DLL (`tools/build_native_color.py`: MSVC `/O2 /fp:precise`). Only the Rust toolchain and
//! a C compiler (MSVC Build Tools on Windows) are needed.

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor").join("lcms2-2.19");
    let mut sources: Vec<PathBuf> = std::fs::read_dir(root.join("src"))
        .expect("vendor/lcms2-2.19/src is missing")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            name.starts_with("cms") && name.ends_with(".c")
        })
        .collect();
    sources.sort();
    let mut build = cc::Build::new();
    build.files(&sources).include(root.join("include")).include(root.join("src")).warnings(false);
    // Optimised in every profile: the oracle's library is an /O2 build, and float results must not depend on
    // whether the Rust crate is built for tests or for release.
    build.opt_level(2).debug(false);
    if build.get_compiler().is_like_msvc() {
        build.flag("/fp:precise");
    } else {
        // Same IEEE semantics elsewhere (clang on macOS, also when cross-building for the other architecture):
        // no fused multiply-add contraction, no fast-math.
        build.flag_if_supported("-ffp-contract=off");
        build.flag_if_supported("-fno-fast-math");
    }
    build.compile("lcms2");
    // For crates that build C code against this LittleCMS (LibRaw in grainy-raw): `DEP_LCMS2_INCLUDE`.
    println!("cargo:include={}", root.join("include").display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor/lcms2-2.19");
}
