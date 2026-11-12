use std::env;
use std::path::PathBuf;

fn main() {
    tauri_build::build();

    println!("cargo:rerun-if-env-changed=LINUXRUNNER_LIB_DIR");
    println!("cargo:rerun-if-changed=../linux/lr_stub.c");
    println!("cargo::rustc-check-cfg=cfg(lr_stub)");

    // Real core: static libraries built by scripts/build-linux-core.sh (lr_embed + LinuxRunner). Found via the
    // environment variable, or in src-tauri/linuxlibs (CI copies them there, because environment variables
    // don't always survive the trip through xcodebuild and the Tauri CLI).
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    let local = manifest.join("linuxlibs");
    println!("cargo:rerun-if-changed={}", local.display());
    let lib_dir = match env::var("LINUXRUNNER_LIB_DIR") {
        Ok(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ if local.join("liblr_embed.a").is_file() => Some(local),
        _ => None,
    };
    match lib_dir {
        Some(dir) => {
            println!("cargo:warning=Linking the real Linux core from {}", dir.display());
            println!("cargo:rustc-link-search=native={}", dir.display());
            for lib in ["lr_embed", "ish", "ish_emu", "fakefs"] {
                println!("cargo:rustc-link-lib=static={lib}");
            }
            println!("cargo:rustc-link-lib=sqlite3");
        }
        // No core available: link the small stand-in so everything still builds and can be tested.
        None => {
            println!("cargo:warning=No Linux core found: linking the STUB terminal");
            cc::Build::new().file("../linux/lr_stub.c").include("../linux").compile("lr_stub");
            println!("cargo:rustc-cfg=lr_stub");
        }
    }
}
