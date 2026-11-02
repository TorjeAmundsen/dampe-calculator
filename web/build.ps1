# Builds the solver for the page. Then web/ is all there is to host, as static files.
#
#     rustup target add wasm32-unknown-unknown
#     ./web/build.ps1
#     python -m http.server --directory web

$ErrorActionPreference = "Stop"
cargo build -p dampe-web --profile dist --target wasm32-unknown-unknown --manifest-path "$PSScriptRoot/../Cargo.toml"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$targetDir = (cargo metadata --format-version 1 --no-deps --manifest-path "$PSScriptRoot/../Cargo.toml" | ConvertFrom-Json).target_directory
Copy-Item "$targetDir/wasm32-unknown-unknown/dist/dampe_web.wasm" "$PSScriptRoot/dampe_web.wasm"
