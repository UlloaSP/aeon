<#
.SYNOPSIS
  Packages a built aeon.exe into an MSI and a portable zip.

.DESCRIPTION
  Expects `cargo build --release --target <triple>` to have run already. CI
  (.github/workflows/release-windows.yml) and local builds use this same
  script, so a packaging change is testable without a release.

  Requires the WiX v5 CLI on PATH and its Util extension:
    dotnet tool install --global wix --version 5.0.2
    wix extension add -g WixToolset.Util.wixext/5.0.2

.EXAMPLE
  cargo build --release --locked --target x86_64-pc-windows-msvc -p aeon
  ./scripts/package-msi.ps1 -Version 0.1.0 -Arch x64
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)]
  [string] $Version,

  [ValidateSet('x64', 'arm64')]
  [string] $Arch = 'x64',

  [string] $OutDir = 'dist'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
$triple = @{ x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }[$Arch]
$binDir = Join-Path $root "target/$triple/release"
$exe = Join-Path $binDir 'aeon.exe'

if (-not (Test-Path $exe)) {
  throw "Missing $exe. Run: cargo build --release --locked --target $triple -p aeon"
}

# MSI versions are three numeric fields, each field at most 255.255.65535.
# A nightly such as 0.2.0-nightly.20261123.4 installs as 0.2.0; the MSI's
# same-version upgrade rule lets it replace the previous nightly.
if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$') {
  throw "Version '$Version' is not semver."
}
$msiVersion = '{0}.{1}.{2}' -f $Matches[1], $Matches[2], $Matches[3]
if ([int]$Matches[1] -gt 255 -or [int]$Matches[2] -gt 255 -or [int]$Matches[3] -gt 65535) {
  throw "Version '$Version' exceeds the MSI limit of 255.255.65535."
}

$wix = Get-Command wix -ErrorAction SilentlyContinue
if (-not $wix) {
  throw 'WiX CLI not found. Install it with: dotnet tool install --global wix --version 5.0.2'
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$name = "aeon-$Version-windows-$Arch"
$msi = Join-Path $OutDir "$name.msi"
$zip = Join-Path $OutDir "$name.zip"

& wix build (Join-Path $root 'packaging/windows/aeon.wxs') `
  -arch $Arch `
  -ext WixToolset.Util.wixext `
  -d "Version=$msiVersion" `
  -d "BinDir=$binDir" `
  -intermediatefolder (Join-Path $root "packaging/windows/obj/$Arch") `
  -o $msi
if ($LASTEXITCODE -ne 0) { throw "wix build failed with exit code $LASTEXITCODE" }

# wix writes a .wixpdb next to the MSI; it is a debugging aid, not a release asset.
Remove-Item -Force -ErrorAction SilentlyContinue ([IO.Path]::ChangeExtension($msi, '.wixpdb'))

Compress-Archive -Force -Path $exe, (Join-Path $root 'LICENSE') -DestinationPath $zip

Get-Item $msi, $zip | ForEach-Object { '{0}  {1:N0} KB' -f $_.Name, ($_.Length / 1KB) }
