#Requires -Version 7.3
[CmdletBinding()]
param(
    [switch]$Check,
    [switch]$Bundle
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$updateConfigPath = $null

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
        if ($env:FERRY_UPDATER_PUBLIC_KEY) {
            $updateConfigPath = Join-Path ([IO.Path]::GetTempPath()) "ferry-updater-$([guid]::NewGuid()).json"
            @{
                bundle = @{ createUpdaterArtifacts = $true }
                plugins = @{ updater = @{ pubkey = $env:FERRY_UPDATER_PUBLIC_KEY } }
            } | ConvertTo-Json -Compress -Depth 5 | Set-Content -LiteralPath $updateConfigPath
            $bundleArgs += @('--config', $updateConfigPath)
        }
    }

    npm.cmd --prefix app run tauri -- build @bundleArgs -- --locked
    cargo build --locked --release -p ferry-cli
}
finally {
    if ($updateConfigPath -and (Test-Path -LiteralPath $updateConfigPath)) {
        Remove-Item -LiteralPath $updateConfigPath
    }
    Pop-Location
}
