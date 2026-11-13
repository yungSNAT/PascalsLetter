# Installs the latest published release with install.ps1, run as a file and
# through `Get-Content -Raw | iex` (the way `irm | iex` runs it), in a child of
# this PowerShell, and checks that each installed rtk.exe is the one in the
# published archive, whose SHA-256 matches the published checksums.txt, and
# that it runs. Needs the network; CI runs it on a clean Windows runner:
#   powershell -NoProfile -File scripts/test-install-live.ps1
#   pwsh -NoProfile -File scripts/test-install-live.ps1

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$installer = (Resolve-Path (Join-Path $PSScriptRoot '..\install.ps1')).Path
$shell = (Get-Process -Id $PID).Path
$asset = 'rtk-oss-x86_64-pc-windows-msvc.zip'
$base = 'https://github.com/yungSNAT/PascalsLetter/releases/latest/download'
$work = Join-Path ([IO.Path]::GetTempPath()) ("rtk-install-live-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null

# The published archive, checked against the published checksum, and the
# SHA-256 of the rtk.exe inside it.
Invoke-WebRequest -Uri "$base/checksums.txt" -OutFile "$work\checksums.txt" -UseBasicParsing
Invoke-WebRequest -Uri "$base/$asset" -OutFile "$work\$asset" -UseBasicParsing
$published = @(Get-Content -LiteralPath "$work\checksums.txt" | Where-Object { $_.EndsWith("  $asset") })
if ($published.Count -ne 1) { throw "checksums.txt has $($published.Count) lines for $asset" }
$archiveHash = (Get-FileHash -LiteralPath "$work\$asset" -Algorithm SHA256).Hash
if ($archiveHash -ne $published[0].Substring(0, 64)) { throw "published archive $archiveHash does not match checksums.txt" }
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead("$work\$asset")
try {
    $stream = $zip.GetEntry('rtk.exe').Open()
    try { $expected = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($stream)) -replace '-' }
    finally { $stream.Dispose() }
} finally { $zip.Dispose() }
Write-Host "published archive matches checksums.txt; its rtk.exe is $expected"

$routes = [ordered]@{
    file = @('-File', $installer)
    iex  = @('-Command', "Get-Content -LiteralPath '$installer' -Raw | Invoke-Expression")
}
$failed = 0
foreach ($route in $routes.Keys) {
    $dir = Join-Path $work "$route-bin"
    $env:RTK_INSTALL_DIR = $dir
    Remove-Item Env:RTK_VERSION -ErrorAction SilentlyContinue
    & $shell -NoProfile -NonInteractive -ExecutionPolicy Bypass @($routes[$route])
    $exit = $LASTEXITCODE
    $exe = Join-Path $dir 'rtk.exe'
    $hash = if (Test-Path -LiteralPath $exe) { (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash } else { 'none' }
    $version = ''
    $versionExit = 'not run'
    if ($hash -ne 'none') {
        try { $version = (& $exe --version) -join ' '; $versionExit = $LASTEXITCODE }
        catch { $version = $_.Exception.GetBaseException().Message }
    }
    if ($exit -eq 0 -and $hash -eq $expected -and $versionExit -eq 0 -and $version -like 'rtk *') {
        Write-Host "$route route: installed $hash, '$version'" -ForegroundColor Green
    } else {
        Write-Host "FAIL: $route route: installer exit $exit, rtk.exe $hash, --version exit $versionExit '$version'" -ForegroundColor Red
        $failed++
    }
}
Remove-Item Env:RTK_INSTALL_DIR
Remove-Item -LiteralPath $work -Recurse -Force
if ($failed) { exit 1 }
Write-Host "install.ps1: both routes installed the published release ($($PSVersionTable.PSEdition) $($PSVersionTable.PSVersion))" -ForegroundColor Green
