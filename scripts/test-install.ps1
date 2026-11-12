# Tests for install.ps1. Run with each PowerShell the installer supports:
#   powershell -NoProfile -File scripts/test-install.ps1   # Windows PowerShell 5.1
#   pwsh -NoProfile -File scripts/test-install.ps1         # PowerShell 7
#
# Each case runs the installer in a child of the same PowerShell, with the
# network replaced by a fixture release, and checks its exit code and whether
# rtk.exe was installed. The fixture rtk.exe is not a program, so a run of it
# would fail the good case.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$installer = (Resolve-Path (Join-Path $PSScriptRoot '..\install.ps1')).Path
$shell = (Get-Process -Id $PID).Path
$asset = 'rtk-oss-x86_64-pc-windows-msvc.zip'
$work = Join-Path ([IO.Path]::GetTempPath()) ("rtk-install-test-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
$failures = @()

# A release folder holding $asset (rtk.exe and LICENSE) and checksums.txt.
function New-Release([string]$Name) {
    $dir = Join-Path $work $Name
    $content = Join-Path $dir 'content'
    New-Item -ItemType Directory -Path $content -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $content 'rtk.exe'), "fixture $Name, not a program")
    [IO.File]::WriteAllText((Join-Path $content 'LICENSE'), 'Apache-2.0')
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory($content, (Join-Path $dir $asset))
    $hash = (Get-FileHash -LiteralPath (Join-Path $dir $asset) -Algorithm SHA256).Hash.ToLower()
    $lines = @(
        (('0' * 64) + '  rtk-oss-x86_64-unknown-linux-musl.tar.gz'),
        "$hash  $asset"
    )
    [IO.File]::WriteAllLines((Join-Path $dir 'checksums.txt'), $lines)
    $dir
}

# Runs the installer against $Release (or a failing network) and returns its
# exit code, the install directory and the URLs it asked for.
function Invoke-Installer([string]$Name, [string]$Release, [switch]$FailDownload) {
    $installDir = Join-Path $work "$Name-bin"
    $urls = Join-Path $work "$Name-urls.txt"
    $download = if ($FailDownload) {
        "throw 'The remote server returned an error: (404) Not Found.'"
    } else {
        "Copy-Item -LiteralPath (Join-Path '$Release' (Split-Path `$Url -Leaf)) -Destination `$OutFile"
    }
    $case = Join-Path $work "$Name.ps1"
    Set-Content -LiteralPath $case -Value @"
. '$installer'
function Get-RtkDownload([string]`$Url, [string]`$OutFile) {
    Add-Content -LiteralPath '$urls' -Value `$Url
    $download
}
Install-Rtk
"@
    $env:RTK_VERSION = 'fork-v0.0.0-test'
    $env:RTK_INSTALL_DIR = $installDir
    @{ Exit = (Invoke-Child $case $Name); Dir = $installDir; Urls = $urls }
}

# Runs a script in a child PowerShell, its output to $Name.log, and returns the
# exit code. Windows PowerShell 5.1 turns a child's stderr into errors, which
# 'Stop' would throw, so this scope continues instead.
function Invoke-Child([string]$Script, [string]$Name) {
    $ErrorActionPreference = 'Continue'
    & $shell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $Script *> (Join-Path $work "$Name.log")
    $LASTEXITCODE
}

function Assert([bool]$Condition, [string]$Message) {
    if (-not $Condition) { $script:failures += $Message; Write-Host "FAIL: $Message" -ForegroundColor Red }
}

function Assert-Refused([string]$Name, $Result) {
    Assert ($Result.Exit -ne 0) "${Name}: exit code $($Result.Exit), expected non-zero"
    Assert (-not (Test-Path -LiteralPath (Join-Path $Result.Dir 'rtk.exe'))) "${Name}: rtk.exe was installed"
}

try {
    # 1. A good archive installs rtk.exe, byte for byte, from this repository's releases only.
    $good = New-Release 'good'
    $r = Invoke-Installer 'good' $good
    Assert ($r.Exit -eq 0) "good: exit code $($r.Exit), expected 0 (see $work\good.log)"
    $installed = Join-Path $r.Dir 'rtk.exe'
    Assert ((Test-Path -LiteralPath $installed) -and
        ([IO.File]::ReadAllText($installed) -eq 'fixture good, not a program')) 'good: rtk.exe not installed as packed'
    Assert (-not (Test-Path -LiteralPath (Join-Path $r.Dir 'LICENSE'))) 'good: an entry other than rtk.exe was written'
    $asked = @(Get-Content -LiteralPath $r.Urls)
    Assert ($asked.Count -eq 2) "good: $($asked.Count) downloads, expected 2"
    foreach ($url in $asked) {
        Assert ($url.StartsWith('https://github.com/yungSNAT/PascalsLetter/releases/download/fork-v0.0.0-test/')) "good: downloaded $url"
    }

    # 2. An altered archive is refused.
    $tampered = New-Release 'tampered'
    [IO.File]::AppendAllText((Join-Path $tampered $asset), 'x')
    Assert-Refused 'tampered' (Invoke-Installer 'tampered' $tampered)

    # 3. A partial download is refused.
    $partial = New-Release 'partial'
    $zip = Join-Path $partial $asset
    $bytes = [IO.File]::ReadAllBytes($zip)
    [IO.File]::WriteAllBytes($zip, $bytes[0..([int]($bytes.Length / 2))])
    Assert-Refused 'partial' (Invoke-Installer 'partial' $partial)

    # 4. checksums.txt without a line for the asset is refused.
    $missing = New-Release 'missing'
    $sums = Join-Path $missing 'checksums.txt'
    [IO.File]::WriteAllLines($sums, @(Get-Content -LiteralPath $sums | Where-Object { $_ -notmatch [regex]::Escape($asset) }))
    Assert-Refused 'missing checksum' (Invoke-Installer 'missing' $missing)

    # 5. An HTTP failure is refused.
    Assert-Refused 'HTTP failure' (Invoke-Installer 'http' $good -FailDownload)

    # 6. Run as a file, a failure exits 1. A malformed tag fails before any download.
    $env:RTK_VERSION = 'not a tag'
    $env:RTK_INSTALL_DIR = Join-Path $work 'file-bin'
    $exit = Invoke-Child $installer 'file'
    Assert ($exit -eq 1) "file mode: exit code $exit, expected 1"
    Assert (-not (Test-Path -LiteralPath $env:RTK_INSTALL_DIR)) 'file mode: install directory created'
} finally {
    Remove-Item Env:RTK_VERSION, Env:RTK_INSTALL_DIR -ErrorAction SilentlyContinue
}

if ($failures.Count -gt 0) {
    Write-Host "$($failures.Count) failure(s); logs in $work" -ForegroundColor Red
    exit 1
}
Remove-Item -LiteralPath $work -Recurse -Force
Write-Host "install.ps1: all 6 cases passed ($($PSVersionTable.PSEdition) $($PSVersionTable.PSVersion))" -ForegroundColor Green
