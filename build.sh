#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

check=false
bundle=false
for arg in "$@"; do
    case "$arg" in
        --check) check=true ;;
        --bundle) bundle=true ;;
        -h|--help)
            echo "Usage: ./build.sh [--check] [--bundle]"
            echo "Builds the desktop app and CLI. --check runs checks and tests; --bundle also builds installers"
            echo "(.dmg and .pkg on macOS, .deb and .AppImage on Linux)."
            exit 0
            ;;
        *) echo "Unknown argument: $arg" >&2; exit 2 ;;
    esac
done

npm --prefix app ci

if "$check"; then
    npm --prefix app run check
    npm --prefix app test
    npm --prefix app run build
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace
    if ! "$bundle"; then
        exit 0
    fi
fi

bundle_args=(--no-bundle)
if "$bundle"; then
    case "$(uname -s)" in
        Darwin) bundle_args=(--bundles app,dmg) ;; # The .pkg is built from the kept .app.
        Linux) bundle_args=(--bundles deb,appimage) ;;
        *) echo "Use build.ps1 to build Windows installers." >&2; exit 2 ;;
    esac
fi

npm --prefix app run tauri -- build "${bundle_args[@]}" -- --locked
if "$bundle" && [[ "$(uname -s)" == Darwin ]]; then
    ./scripts/macos-pkg.sh target/release/bundle/macos/Ferry.app target/release/bundle/pkg
fi
cargo build --locked --release -p ferry-cli
