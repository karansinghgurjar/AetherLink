Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Set-Location "$PSScriptRoot\..\host-rust"

if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
    $cargoPath = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
    if (Test-Path $cargoPath) {
        $env:Path = "$(Split-Path $cargoPath);$env:Path"
    }
}

cargo build --release
Write-Host "Host release build complete: host-rust\target\release\host-rust.exe"
