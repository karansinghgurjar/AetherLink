Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Set-Location "$PSScriptRoot\..\remote_client"
$flutter = Join-Path $env:USERPROFILE "flutter\bin\flutter.bat"

if (-not (Test-Path $flutter)) {
    throw "Flutter not found at $flutter"
}

& $flutter pub get
& $flutter build apk --release

Write-Host "Android release build complete: remote_client\build\app\outputs\flutter-apk\app-release.apk"
