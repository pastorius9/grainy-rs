use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn sources(dir: &Path, ext: &str, recursive: bool) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for entry in fs::read_dir(dir).expect("vendored source directory").flatten() {
        let path = entry.path();
        if path.is_dir() && recursive {
            result.extend(sources(&path, ext, true));
        } else if path.extension().is_some_and(|x| x == ext) {
            result.push(path);
        }
    }
    result.sort();
    result
}
fn base(out: &Path, cpp: bool) -> cc::Build {
    let mut b = cc::Build::new();
    b.cpp(cpp).include(out).warnings(false).opt_level(2);
    b.define("_CRT_SECURE_NO_WARNINGS", None);
    let symbols = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/native_symbols.h");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        b.flag(format!("/FI{}", symbols.display()));
        b.flag("/fp:precise");
        if cpp {
            b.flag("/EHsc");
        }
    } else {
        b.flag("-include").flag(symbols.to_str().expect("native header path"));
        b.flag("-fno-fast-math").flag("-ffp-contract=off");
    }
    b
}
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/shim.cpp");
    println!("cargo:rerun-if-changed=src/native_symbols.h");
    println!("cargo:rerun-if-changed=src/serial_libraw.h");
    println!("cargo:rerun-if-changed=vendor");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let jpg = env::var_os("DEP_GRAINY_JPEG_INCLUDE").expect("grainy-jpeg-sys include path");
    println!("cargo:rerun-if-env-changed=DEP_GRAINY_JPEG_INCLUDE");
    // Compile the shim/LibRaw before its static dependencies for Unix link ordering.
    let raw = Path::new("vendor/LibRaw-0.22.1");
    let z = Path::new("vendor/zlib-1.3.1");
    let cms_include = env::var_os("DEP_LCMS2_INCLUDE").expect("grainy-cms must export its LittleCMS include path");
    println!("cargo:rerun-if-env-changed=DEP_LCMS2_INCLUDE");
    let configured = || {
        let mut b = base(&out, true);
        b.include(raw)
            .include(&jpg)
            .include(z)
            .include(&cms_include)
            .define("LIBRAW_NODLL", None)
            .define("USE_JPEG", None)
            .define("USE_ZLIB", None)
            .define("USE_LCMS2", None);
        b
    };
    // Only these measured, independently tiled paths enable OpenMP. In particular,
    // xtrans_demosaic.cpp has no parallel regions, independent of OMP_NUM_THREADS.
    // Other platforms retain the serial build until their toolchains are verified.
    let msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let parallel_names = ["ahd_demosaic.cpp", "crx.cpp", "fuji_compressed.cpp"];
    let (parallel, serial): (Vec<_>, Vec<_>) = sources(&raw.join("src"), "cpp", true)
        .into_iter()
        .filter(|p| !p.file_name().unwrap().to_string_lossy().ends_with("_ph.cpp"))
        .partition(|p| msvc && parallel_names.iter().any(|name| p.file_name().is_some_and(|n| n == *name)));
    let mut raw_build = configured();
    if msvc {
        // LibRaw's inline allocator must have identical, synchronized definitions
        // in every translation unit. Load that header with OpenMP enabled first,
        // then disable only the source-file parallel regions in the serial group.
        let serial_header = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/serial_libraw.h");
        raw_build.flag("/openmp").flag(format!("/FI{}", serial_header.display()));
    }
    raw_build.files(serial).file("src/shim.cpp");
    raw_build.compile("grainy_libraw");
    if !parallel.is_empty() {
        configured().flag("/openmp").files(parallel).compile("grainy_libraw_parallel");
    }
    let mut zb = base(&out, false);
    zb.include(z).include(&jpg);
    for name in "adler32 compress crc32 deflate infback inffast inflate inftrees trees uncompr zutil".split_whitespace()
    {
        zb.file(z.join(format!("{name}.c")));
    }
    zb.compile("grainy_raw_zlib");
}
