# Rebuild every plugin component and refresh plugins/prebuilt/.
#
# Needs wasi-sdk 34 (docs/wasm-toolchain.md) for the tree-sitter grammars,
# which are C. Point WASI_SDK_PATH at it, or pass -Sdk.
param([string]$Sdk = $env:WASI_SDK_PATH, [string[]]$Only = @())
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
if (-not $Sdk) { $Sdk = "D:\tools\wasi-sdk-34.0-x86_64-windows" }
$env:WASI_SDK_PATH = $Sdk
$env:CC_wasm32_wasip2 = "$Sdk\bin\clang.exe"
$env:AR_wasm32_wasip2 = "$Sdk\bin\llvm-ar.exe"
$env:CFLAGS_wasm32_wasip2 = "--sysroot=$Sdk/share/wasi-sysroot"
# Reproducible bytes: no absolute paths of this checkout in the artifact.
$root = (Resolve-Path "$PSScriptRoot\..").Path
$env:RUSTFLAGS = "--remap-path-prefix=$root=/ra --remap-path-prefix=$env:USERPROFILE=/home"
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = "3" }
New-Item -ItemType Directory -Force prebuilt | Out-Null
$out = "target\wasm32-wasip2\release"

function Build($package, $feature, $artifact) {
  if ($Only.Count -gt 0 -and -not ($Only -contains $artifact)) { return }
  $args = @("build", "--release", "--target", "wasm32-wasip2", "-p", $package)
  if ($feature) { $args += @("--features", $feature) }
  & cargo @args
  if ($LASTEXITCODE -ne 0) { throw "build of $artifact failed" }
  Copy-Item "$out\$package.wasm" "prebuilt\$artifact.wasm" -Force
}

Build "probe" $null "probe"
Build "probe3" $null "probe3"
# One component per language family, named after its plugin id.
foreach ($lang in "rust", "ecmascript", "python", "csharp", "go", "java") {
  Build "treesitter" $lang $lang
}
# The bodies of C# members (contract 0.3): the same crate, its own component.
Build "treesitter" "csharp-body" "csharp-body"
# Built like a third party's plugin: its own workspace, the published WIT only.
if ($Only.Count -eq 0 -or $Only -contains "sql") {
  Push-Location "$PSScriptRoot\..\examples\plugin-sql"
  & cargo build --release --target wasm32-wasip2
  if ($LASTEXITCODE -ne 0) { Pop-Location; throw "build of sql failed" }
  Pop-Location
  Copy-Item "$PSScriptRoot\..\examples\plugin-sql\target\wasm32-wasip2\release\plugin_sql.wasm" "prebuilt\sql.wasm" -Force
}

$sums = @()
Get-ChildItem prebuilt -Filter *.wasm | Sort-Object Name | ForEach-Object {
  $h = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
  $sums += "$h  $($_.Name)"
  "$($_.Name): $h ($([math]::Round($_.Length/1KB)) KB)"
}
# LF, as .gitattributes stores it, so a rebuild that changed nothing leaves the
# working tree clean.
[IO.File]::WriteAllText("$PSScriptRoot\prebuilt\SHA256SUMS", ($sums -join "`n") + "`n")
