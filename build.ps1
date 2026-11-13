#Requires -Version 7.3
[CmdletBinding()]
param(
    [switch]$Check,
    [switch]$Bundle
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Push-Location -LiteralPath $PSScriptRoot
try {
    npm.cmd --prefix app ci

    if ($Check) {
        npm.cmd --prefix app run check
        npm.cmd --prefix app test
        npm.cmd --prefix app run build
        cargo fmt --all --check
        cargo clippy --locked --workspace --all-targets -- -D warnings
        cargo test --locked --workspace
        if (-not $Bundle) {
            return
        }
    }

    $bundleArgs = @('--no-bundle')
    if ($Bundle) {
        $bundleArgs = @('--bundles', 'msi,nsis')
    }

    npm.cmd --prefix app run tauri -- build @bundleArgs -- --locked
    cargo build --locked --release -p ferry-cli
}
finally {
    Pop-Location
}
