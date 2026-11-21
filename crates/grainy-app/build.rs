fn main() {
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=native/hdr_display.cpp");
        cc::Build::new()
            .cpp(true)
            .std("c++17")
            .flag_if_supported("/EHsc")
            .file("native/hdr_display.cpp")
            .compile("grainy_hdr_view");
        for lib in ["d3d11", "dxgi", "d3dcompiler", "user32", "gdi32"] {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}
