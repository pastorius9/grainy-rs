use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let jpg = root.join("vendor/libjpeg-turbo-2.1.5.1");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    fs::write(
        out.join("jversion.h"),
        fs::read_to_string(jpg.join("jversion.h.in"))
            .unwrap()
            .replace("@COPYRIGHT_YEAR@", "2023")
            .replace("@VERSION@", "2.1.5.1"),
    )
    .unwrap();
    fs::write(out.join("jconfig.h"), "#define JPEG_LIB_VERSION 62\n#define LIBJPEG_TURBO_VERSION 2.1.5.1\n#define LIBJPEG_TURBO_VERSION_NUMBER 2001005\n#define C_ARITH_CODING_SUPPORTED 1\n#define D_ARITH_CODING_SUPPORTED 1\n#define MEM_SRCDST_SUPPORTED 1\n#define BITS_IN_JSAMPLE 8\n#ifdef _WIN32\n#ifndef __RPCNDR_H__\ntypedef unsigned char boolean;\n#endif\n#define HAVE_BOOLEAN\n#endif\n").unwrap();
    fs::write(out.join("jconfigint.h"), format!("#define BUILD \"grainy\"\n#define INLINE {}\n#define THREAD_LOCAL {}\n#define PACKAGE_NAME \"libjpeg-turbo\"\n#define VERSION \"2.1.5.1\"\n#define SIZEOF_SIZE_T {}\n#define FALLTHROUGH\n", if windows { "__inline" } else { "inline" }, if windows { "__declspec(thread)" } else { "__thread" }, env::var("CARGO_CFG_TARGET_POINTER_WIDTH").unwrap().parse::<u32>().unwrap()/8)).unwrap();
    // A single exported include directory contains the original public headers and generated config.
    for name in ["jpeglib.h", "jmorecfg.h", "jpegint.h", "jerror.h"] {
        fs::copy(jpg.join(name), out.join(name)).unwrap();
    }
    fs::copy(root.join("src/native_symbols.h"), out.join("grainy_jpeg_symbols.h")).unwrap();
    let mut build = cc::Build::new();
    build.include(&out).include(&jpg).warnings(false).opt_level(2);
    build.define("_CRT_SECURE_NO_WARNINGS", None);
    let symbols = out.join("grainy_jpeg_symbols.h");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.flag(format!("/FI{}", symbols.display())).flag("/fp:precise");
    } else {
        build.flag("-include").flag(symbols.to_str().expect("native header path"));
        build.flag("-fno-fast-math").flag("-ffp-contract=off");
    }
    // Scalar code and flags retained from grainy-raw: lossy DNG pixels must not change.
    for name in "jcicc jdicc jcapimin jcapistd jccoefct jccolor jcdctmgr jchuff jcinit jcmainct jcmarker jcmaster jcomapi jcparam jcphuff jcprepct jcsample jctrans jdapimin jdapistd jdatadst jdatasrc jdcoefct jdcolor jddctmgr jdhuff jdinput jdmainct jdmarker jdmaster jdmerge jdphuff jdpostct jdsample jdtrans jerror jfdctflt jfdctfst jfdctint jidctflt jidctfst jidctint jidctred jquant1 jquant2 jutils jmemmgr jmemnobs jaricom jcarith jdarith jsimd_none".split_whitespace() {
        build.file(jpg.join(format!("{name}.c")));
    }
    build.file("src/shim.c").compile("grainy_jpeg");
    // Generator-only coefficient transcoder. Never part of the runtime library/API.
    println!("cargo:rerun-if-env-changed=GRAINY_JPEG_FIXTURE_TOOL");
    if let Some(destination) = env::var_os("GRAINY_JPEG_FIXTURE_TOOL") {
        let compiler = build.get_compiler();
        let mut command = compiler.to_command();
        command.current_dir(&out);
        if compiler.is_like_msvc() {
            command
                .arg("/nologo")
                .arg("/MD")
                .arg(format!("/I{}", out.display()))
                .arg(format!("/Fe{}", PathBuf::from(destination).display()))
                .arg(root.join("tools/arithmetic.c"))
                .arg(out.join("grainy_jpeg.lib"));
        } else {
            command
                .arg("-I")
                .arg(&out)
                .arg(root.join("tools/arithmetic.c"))
                .arg(out.join("libgrainy_jpeg.a"))
                .arg("-o")
                .arg(destination);
        }
        let output = command.output().expect("compile JPEG fixture transcoder");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    println!("cargo:include={}", out.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=tools");
    println!("cargo:rerun-if-changed=vendor");
}
