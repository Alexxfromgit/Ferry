fn main() {
    // Embed the current icon even when only the image files changed (the app icon is compiled
    // into the executable on Windows).
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
