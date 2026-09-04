fn main() {
    #[cfg(target_os = "windows")]
    {
        // Tauri's mock runtime imports TaskDialogIndirect before test main.
        // Embed the same Common Controls v6 dependency as the production binary.
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
    tauri_build::build();
}
