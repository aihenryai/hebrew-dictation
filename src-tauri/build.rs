fn main() {
    // tauri-build's own manifest embedding (via tauri_winres's resource compiler)
    // only reaches the real `hebrew-dictation` bin target, not the separate
    // synthetic executable `cargo test` builds from src/lib.rs — so a lib unit
    // test that calls `tauri::test::mock_app()` (dev-dependency `tauri` "test"
    // feature) fails to even launch with STATUS_ENTRYPOINT_NOT_FOUND, before any
    // test code runs. Confirmed, maintainer-acknowledged upstream Tauri bug with
    // no in-crate fix: https://github.com/orgs/tauri-apps/discussions/11179 and
    // https://github.com/tauri-apps/tauri/pull/4383#issuecomment-1212221864.
    //
    // Fix: opt out of tauri-build's own manifest embedding and instead embed the
    // *identical* manifest ourselves via a plain linker arg, which Cargo applies
    // uniformly to every binary-like artifact built from this package (the real
    // bin, that bin's own unit tests, AND the lib's unit tests alike) — one
    // source of truth, no risk of a duplicate-resource link error, and no
    // behavior change for the shipped app (byte-identical manifest content,
    // verified against tauri-build 2.5.6's own default `windows-app-manifest.xml`).
    #[cfg(windows)]
    let attributes = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    #[cfg(not(windows))]
    let attributes = tauri_build::Attributes::new();

    tauri_build::try_build(attributes).expect("failed to run tauri-build");

    #[cfg(windows)]
    embed_windows_manifest();
}

#[cfg(windows)]
fn embed_windows_manifest() {
    let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("windows-app-manifest.xml");

    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.to_str().unwrap());
}
