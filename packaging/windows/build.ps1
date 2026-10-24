# Build the Windows installer.  Requires: Rust (MSVC), Inno Setup 6 (iscc on PATH).
#   powershell -ExecutionPolicy Bypass -File packaging\windows\build.ps1
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value
cargo build --release -p proart-app -p proart-cli
# the CLI goes in bin\: Windows names are case-insensitive, so ProArt.exe and
# proart.exe cannot sit in the same folder
New-Item -ItemType Directory -Force -Path dist\bin | Out-Null
Copy-Item target\release\proart-app.exe dist\ProArt.exe -Force
Copy-Item target\release\proart.exe dist\bin\proart.exe -Force
if (Get-Command iscc -ErrorAction SilentlyContinue) {
    iscc /DAppVersion=$version packaging\windows\proart.iss
    Write-Host "installer: dist\ProArt-Setup-$version.exe"
} else {
    Write-Host "Inno Setup (iscc) not found: portable files are in dist\ (ProArt.exe, bin\proart.exe)"
}
