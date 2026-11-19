//! Lists the Lensfun XML files in `data/lensfun/` so that `builtin.rs` can embed them in the binary.
use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let folder = manifest.join("data").join("lensfun");
    println!("cargo:rerun-if-changed={}", folder.display());
    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".xml"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut out = String::from("/// The embedded Lensfun XML files, sorted by name: (file name, text).\npub(crate) const FILES: &[(&str, &str)] = &[\n");
    for name in names {
        let path = folder.join(&name);
        println!("cargo:rerun-if-changed={}", path.display());
        let path = path.to_string_lossy().replace('\\', "/");
        let _ = writeln!(out, "    ({name:?}, include_str!({path:?})),");
    }
    out.push_str("];\n");
    let target = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("builtin_files.rs");
    std::fs::write(target, out).expect("write builtin_files.rs");
}
