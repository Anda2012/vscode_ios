use std::env;
use std::path::PathBuf;

fn main() {
    tauri_build::build();

    println!("cargo:rerun-if-env-changed=LINUXRUNNER_LIB_DIR");
    println!("cargo:rerun-if-changed=../linux/lr_stub.c");
    println!("cargo::rustc-check-cfg=cfg(lr_stub)");

    match env::var("LINUXRUNNER_LIB_DIR") {
        // Real core: static libraries built by ios/scripts/build-linux-core.sh (lr_embed + LinuxRunner).
        Ok(dir) if !dir.is_empty() => {
            println!("cargo:rustc-link-search=native={}", PathBuf::from(dir).display());
            for lib in ["lr_embed", "ish", "ish_emu", "fakefs"] {
                println!("cargo:rustc-link-lib=static={lib}");
            }
            println!("cargo:rustc-link-lib=sqlite3");
        }
        // No core available: link the small stand-in so everything still builds and can be tested.
        _ => {
            cc::Build::new().file("../linux/lr_stub.c").include("../linux").compile("lr_stub");
            println!("cargo:rustc-cfg=lr_stub");
        }
    }
}
