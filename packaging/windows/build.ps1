# Build the Windows installer.  Requires: Rust (MSVC), Inno Setup 6 (iscc on PATH).
#   powershell -ExecutionPolicy Bypass -File packaging\windows\build.ps1
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value
cargo build --release -p proart-app -p proart-cli
New-Item -ItemType Directory -Force -Path dist | Out-Null
Copy-Item target\release\proart-app.exe dist\ProArt.exe -Force
Copy-Item target\release\proart.exe dist\proart.exe -Force
if (Get-Command iscc -ErrorAction SilentlyContinue) {
    iscc /DAppVersion=$version packaging\windows\proart.iss
    Write-Host "installer: dist\ProArt-Setup-$version.exe"
} else {
    Write-Host "Inno Setup (iscc) not found: portable files are in dist\ (ProArt.exe, proart.exe)"
}
