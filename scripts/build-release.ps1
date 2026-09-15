# Builds both release artefacts: the portable bundle in .\dist and the Windows
# installer.
#
#   pwsh scripts\build-release.ps1
#
# Order matters. tauri.conf.json ships the reference plugins as bundle
# resources, and Cargo checks those paths while compiling the app, so the plugin
# DLLs have to be built and copied into their plugin directories before the app
# is built. The portable bundle is assembled from those same artefacts.

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root

# Keep scratch work off C:. The workspace target dir lives on the same volume as
# the checkout so linking never fills the system drive.
$scratch = "D:\cargo-tmp"
New-Item -ItemType Directory -Force -Path $scratch | Out-Null
$env:TEMP = $scratch
$env:TMP = $scratch
$env:CARGO_TARGET_DIR = Join-Path $root "target"

$dist = Join-Path $root "dist"
$release = Join-Path $root "target\release"

$plugins = @(
    @{ dir = "poddies-plugin-stats";           dll = "poddies_plugin_stats.dll";           id = "dev.poddies.stats" },
    @{ dir = "poddies-plugin-apple-podcasts";  dll = "poddies_plugin_apple_podcasts.dll";  id = "dev.poddies.apple-podcasts" },
    @{ dir = "poddies-plugin-eq";              dll = "poddies_plugin_eq.dll";              id = "dev.poddies.eq" },
    @{ dir = "poddies-plugin-compressor";      dll = "poddies_plugin_compressor.dll";      id = "dev.poddies.compressor" },
    @{ dir = "poddies-plugin-listening-clock"; dll = "poddies_plugin_listening_clock.dll"; id = "dev.poddies.listening-clock" },
    @{ dir = "poddies-plugin-night-listening"; dll = "poddies_plugin_night_listening.dll"; id = "dev.poddies.night-listening" }
)

Write-Host "==> frontend" -ForegroundColor Cyan
Push-Location (Join-Path $root "app")
pnpm install
pnpm build
Pop-Location

Write-Host "==> reference plugin DLLs" -ForegroundColor Cyan
cargo build --release -p poddies-plugin-stats -p poddies-plugin-apple-podcasts -p poddies-plugin-eq -p poddies-plugin-compressor -p poddies-plugin-listening-clock -p poddies-plugin-night-listening
if ($LASTEXITCODE -ne 0) { throw "building the plugins failed" }
foreach ($plugin in $plugins) {
    $source = Join-Path $release $plugin.dll
    if (-not (Test-Path $source)) { throw "expected $source" }
    Copy-Item $source (Join-Path $root "plugins\$($plugin.dir)") -Force
}

Write-Host "==> application" -ForegroundColor Cyan
cargo build --release -p poddies
if ($LASTEXITCODE -ne 0) { throw "building poddies failed" }

Write-Host "==> assembling dist" -ForegroundColor Cyan
Remove-Item -Recurse -Force $dist -ErrorAction SilentlyContinue
foreach ($plugin in $plugins) {
    New-Item -ItemType Directory -Force -Path (Join-Path $dist "plugins\$($plugin.id)") | Out-Null
    Copy-Item (Join-Path $release $plugin.dll) (Join-Path $dist "plugins\$($plugin.id)")
    Copy-Item (Join-Path $root "plugins\$($plugin.dir)\plugin.json") (Join-Path $dist "plugins\$($plugin.id)")
}
New-Item -ItemType Directory -Force -Path (Join-Path $dist "python") | Out-Null
Copy-Item (Join-Path $root "python\poddies") (Join-Path $dist "python") -Recurse
Copy-Item (Join-Path $release "poddies.exe") $dist
Copy-Item (Join-Path $root "README.md") $dist
Copy-Item (Join-Path $root "LICENSE") $dist
Copy-Item (Join-Path $root "docs") $dist -Recurse

Write-Host "==> installer" -ForegroundColor Cyan
Push-Location (Join-Path $root "app")
pnpm tauri build
if ($LASTEXITCODE -ne 0) { throw "tauri build failed" }
Pop-Location

$bundle = Join-Path $env:CARGO_TARGET_DIR "release\bundle\nsis"
$installer = Get-ChildItem $bundle -Filter "*.exe" | Select-Object -First 1
if (-not $installer) { throw "no installer was produced in $bundle" }
Copy-Item $installer.FullName $dist

Write-Host ""
Write-Host "portable  : $dist\poddies.exe" -ForegroundColor Green
Write-Host "installer : $($installer.FullName)" -ForegroundColor Green
