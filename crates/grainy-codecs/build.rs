use std::{env, fs, path::Path};

const DAV: &str = "vendor/dav1d";
const AVIF: &str = "vendor/libavif";

fn main() {
    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-changed={DAV}");
    println!("cargo:rerun-if-changed={AVIF}");
    let windows = env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows";
    let msvc = env::var("CARGO_CFG_TARGET_ENV").unwrap() == "msvc";
    let root = Path::new(DAV);
    let sources = |dir: &Path| {
        let mut list: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "c"))
            .collect();
        list.sort();
        list
    };
    let mut container = cc::Build::new();
    container
        .std("c11")
        .warnings(false)
        .include(format!("{AVIF}/include"))
        .include(root.join("include"))
        .define("AVIF_CODEC_DAV1D", "1");
    // Keep the codec and container entirely static; no system image extensions are required.
    let avif_sources = sources(&Path::new(AVIF).join("src"));
    if msvc {
        container.flag("/utf-8");
    }
    container
        .files(
            avif_sources
                .iter()
                .filter(|p| !matches!(p.file_name().and_then(|n| n.to_str()), Some("scale.c" | "sampletransform.c"))),
        )
        .file("native/avif_reader.c")
        .compile("grainy_avif");
    let mut decoder = cc::Build::new();
    decoder.std("c11").warnings(false).include("native/dav1d").include(root).include(root.join("include"));
    if msvc {
        decoder.flag("/utf-8");
        decoder.include(root.join("include/compat/msvc"));
    }
    if !windows {
        decoder.define("HAVE_POSIX_MEMALIGN", "1").define("HAVE_UNISTD_H", "1").define("_GNU_SOURCE", "1");
    }
    let (templates, plain): (Vec<_>, Vec<_>) = sources(&root.join("src"))
        .into_iter()
        .partition(|p| p.file_stem().unwrap().to_string_lossy().ends_with("_tmpl"));
    for depth in ["8", "16"] {
        decoder.clone().define("BITDEPTH", depth).files(&templates).compile(&format!("grainy_dav1d_{depth}"));
    }
    if windows {
        decoder.file(root.join("src/win32/thread.c"));
    }
    decoder.files(plain).compile("grainy_dav1d");
}
