//! Builds the vendored libopus (float build, CELT + SILK) with cc.

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("vendor/opus");

    let mut files = Vec::new();
    let mut dir = |folder: &str, skip: &[&str]| {
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(root.join(folder)).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.extension().is_some_and(|e| e == "c") && !skip.contains(&name.as_str()) {
                paths.push(path);
            }
        }
        paths.sort();
        files.extend(paths);
    };
    // CELT
    dir("celt", &["opus_custom_demo.c"]);
    // SILK common and the float build
    dir("silk", &[]);
    dir("silk/float", &[]);
    // src
    dir(
        "src",
        &["opus_compare.c", "opus_demo.c", "repacketizer_demo.c"],
    );

    let mut build = cc::Build::new();
    build
        .files(&files)
        .define("OPUS_BUILD", "")
        .define("OPUS_VERSION", "\"\\\"horizon-voice vendored opus 1.5.2\\\"\"")
        .define("VAR_ARRAYS", "1")
        .define("HAVE_LRINT", "1")
        .define("HAVE_LRINTF", "1")
        .include(root.join("include"))
        .include(root.join("celt"))
        .include(root.join("silk"))
        .include(root.join("silk/float"))
        .include(root.join("src"))
        .flag_if_supported("-fno-math-errno")
        .warnings(false);

    // Termux's bionic headers reject versionless android triples, and an NDK
    // clang takes the last flag: pin the triple's API level for every host.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
        let triple = match arch.as_str() {
            "aarch64" => "aarch64-linux-android24",
            "x86_64" => "x86_64-linux-android24",
            "x86" => "i686-linux-android24",
            "arm" => "armv7a-linux-androideabi24",
            other => {
                eprintln!("voice: unknown android arch {other}, leaving cc defaults");
                ""
            }
        };
        if !triple.is_empty() {
            build.flag(format!("--target={triple}"));
        }
    }
    build.compile("opus");
    println!("cargo:rerun-if-changed=vendor/opus");
}
