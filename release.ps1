#!/usr/bin/env pwsh
# Builds release zips into dist/.
#
#   ./release.ps1                  every target
#   ./release.ps1 windows-x64      only this one, or linux-x64
#
# Same OS as this machine: plain cargo. Linux from Windows: cargo-zigbuild and Zig. Windows from
# Linux isn't set up, and is skipped.

$ErrorActionPreference = "Stop"

$targets = [ordered]@{
    "windows-x64" = "x86_64-pc-windows-msvc"
    "linux-x64"   = "x86_64-unknown-linux-gnu"
}

# Oldest glibc Linux builds support when cross-compiled
$glibcVersion = "2.35"

$cargoToml = Get-Content "$PSScriptRoot/Cargo.toml" -Raw
if ($cargoToml -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
    Write-Error "Couldn't find [workspace.package] version in Cargo.toml"
}
$version = $Matches[1]
$targetDir = (cargo metadata --format-version 1 --no-deps --manifest-path "$PSScriptRoot/Cargo.toml" | ConvertFrom-Json).target_directory

function Invoke-Cargo($cargoArgs) {
    Write-Host "  cargo $($cargoArgs -join ' ')"
    & cargo @cargoArgs | Out-Host
    if ($LASTEXITCODE -ne 0) { Write-Error "cargo failed with exit code $LASTEXITCODE" }
}

function RequireZig() {
    # cargo-zigbuild finds Zig on PATH or as the ziglang Python package
    if (Get-Command "zig" -ErrorAction SilentlyContinue) { return }
    foreach ($python in @("python3", "python")) {
        if (Get-Command $python -ErrorAction SilentlyContinue) {
            & $python -m ziglang version *> $null
            if ($LASTEXITCODE -eq 0) { return }
        }
    }
    Write-Error "Zig is needed to build Linux on Windows. Install it from https://ziglang.org/download, or run: pip install ziglang"
}

function RequireZigbuild() {
    if (-not (Get-Command "cargo-zigbuild" -ErrorAction SilentlyContinue)) {
        Write-Host "  Installing cargo-zigbuild..."
        cargo install --locked cargo-zigbuild | Out-Host
        if ($LASTEXITCODE -ne 0) { Write-Error "Installing cargo-zigbuild failed" }
    }
}

# Compress-Archive drops the executable bit, so entries are added with Unix permissions set
function Zip($directory, $zip, $executable) {
    Add-Type -AssemblyName System.IO.Compression
    $stream = [System.IO.File]::Open($zip, [System.IO.FileMode]::Create)
    $archive = [System.IO.Compression.ZipArchive]::new($stream, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($file in Get-ChildItem $directory -File) {
            $entry = $archive.CreateEntry($file.Name, [System.IO.Compression.CompressionLevel]::Optimal)
            $mode = if ($file.Name -eq $executable) { 0x81ED } else { 0x81A4 }
            $entry.ExternalAttributes = $mode -shl 16
            $writer = $entry.Open()
            try {
                $reader = [System.IO.File]::OpenRead($file.FullName)
                try { $reader.CopyTo($writer) } finally { $reader.Dispose() }
            }
            finally { $writer.Dispose() }
        }
    }
    finally {
        $archive.Dispose()
        $stream.Dispose()
    }
}

function Build($name) {
    Write-Host "Building $name..."
    $triple = $targets[$name]
    $isWindowsTarget = $name -like "windows-*"
    if ($isWindowsTarget -and -not $IsWindows) {
        Write-Warning "Skipping ${name}: it only builds on Windows here."
        return
    }
    rustup target add $triple | Out-Host

    $common = @("--profile", "dist", "-p", "dampe")
    if ($isWindowsTarget) {
        Invoke-Cargo (@("build", "--target", $triple, "--features", "windowed") + $common)
    }
    elseif ($IsLinux) {
        Invoke-Cargo (@("build", "--target", $triple) + $common)
    }
    else {
        RequireZig
        RequireZigbuild
        # Zig's linker says it ignores the -O1 that rustc passes, which rustc shows as a warning
        $rustflags = "CARGO_TARGET_$($triple.ToUpper() -replace '-', '_')_RUSTFLAGS"
        $previous = [Environment]::GetEnvironmentVariable($rustflags)
        [Environment]::SetEnvironmentVariable($rustflags, "-A linker-messages")
        try {
            Invoke-Cargo (@("zigbuild", "--target", "$triple.$glibcVersion") + $common)
        }
        finally {
            [Environment]::SetEnvironmentVariable($rustflags, $previous)
        }
    }

    $binary = if ($isWindowsTarget) { "dampe.exe" } else { "dampe" }
    $output = "$PSScriptRoot/dist/$name"
    if (Test-Path $output) { Remove-Item $output -Recurse -Force }
    New-Item -ItemType Directory -Path $output | Out-Null
    Copy-Item "$targetDir/$triple/dist/$binary" $output
    Copy-Item "$PSScriptRoot/README.md", "$PSScriptRoot/LICENSE" $output

    $zip = "$PSScriptRoot/dist/dampe-calculator-$version-$name.zip"
    Zip $output $zip $binary
    Write-Host "  Zipped: $zip"
}

$selected = if ($args.Count -gt 0) { $args } else { $targets.Keys }
foreach ($name in $selected) {
    if (-not $targets.Contains($name)) {
        Write-Error "Unknown target '$name'. Targets: $($targets.Keys -join ', ')"
    }
}
foreach ($name in @($selected)) { Build $name }
Write-Host "Build complete."
