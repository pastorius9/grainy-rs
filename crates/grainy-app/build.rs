fn main() {
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=native/grainy.ico");
        println!("cargo:rerun-if-changed=native/grainy.rc");
        let resource = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("grainy.res");
        let target = std::env::var("TARGET").unwrap();
        let mut rc =
            cc::windows_registry::find(&target, "rc.exe").unwrap_or_else(|| std::process::Command::new("rc.exe"));
        let status = rc
            .current_dir("native")
            .args(["/nologo", "/fo"])
            .arg(&resource)
            .arg("grainy.rc")
            .status()
            .expect("run the Windows SDK resource compiler");
        assert!(status.success(), "compile the Grainy application icon");
        println!("cargo:rustc-link-arg-bin=grainy-app={}", resource.display());
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
