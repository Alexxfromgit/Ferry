fn main() {
    // Embed the current icon even when only the image files changed (the app icon is compiled
    // into the executable on Windows).
    println!("cargo:rerun-if-changed=icons");
    distribution();
    tauri_build::build()
}

/// Which channel this build ships through, as `FERRY_DISTRIBUTION` for `env!`. Android builds
/// get it from the Gradle property `ferryDist` (see `gen/android/buildSrc`).
fn distribution() {
    println!("cargo:rerun-if-env-changed=FERRY_DISTRIBUTION");
    let requested = std::env::var("FERRY_DISTRIBUTION").ok();
    let distribution = match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("android") => match requested.as_deref().unwrap_or("play") {
            value @ ("play" | "direct") => value.to_owned(),
            other => panic!("FERRY_DISTRIBUTION must be play or direct on Android, not {other}"),
        },
        Ok("ios") => "appstore".to_owned(),
        _ => "desktop".to_owned(),
    };
    println!("cargo:rustc-env=FERRY_DISTRIBUTION={distribution}");
}
