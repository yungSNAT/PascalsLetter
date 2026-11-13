# Tests for install.ps1. Run with each PowerShell the installer supports:
#   powershell -NoProfile -File scripts/test-install.ps1   # Windows PowerShell 5.1
#   pwsh -NoProfile -File scripts/test-install.ps1         # PowerShell 7
#
# Each case runs the installer in a child of the same PowerShell, with the
# network replaced by a fixture release, and checks its exit code, its message,
# whether rtk.exe was installed and that its temporary files are gone. Each
# guard in install.ps1 has a case that fails when that guard alone is removed,
# except Set-StrictMode, which only turns a slip in the script into an error.
# The fixture rtk.exe is a copy of Windows' curl.exe, which answers --version.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$installer = (Resolve-Path (Join-Path $PSScriptRoot '..\install.ps1')).Path
$shell = (Get-Process -Id $PID).Path
$asset = 'rtk-oss-x86_64-pc-windows-msvc.zip'
$program = Join-Path $env:SystemRoot 'System32\curl.exe'
$work = Join-Path ([IO.Path]::GetTempPath()) ("rtk-install-test-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
$failures = @()

# A release folder holding $asset, with $Program packed as $Entry beside a
# LICENSE, and checksums.txt.
function New-Release([string]$Name, [string]$Program = $program, [string]$Entry = 'rtk.exe') {
    $dir = Join-Path $work $Name
    $entryPath = Join-Path (Join-Path $dir 'content') $Entry
    New-Item -ItemType Directory -Path (Split-Path $entryPath) -Force | Out-Null
    Copy-Item -LiteralPath $Program -Destination $entryPath
    [IO.File]::WriteAllText((Join-Path $dir 'content\LICENSE'), 'Apache-2.0')
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory((Join-Path $dir 'content'), (Join-Path $dir $asset))
    $hash = (Get-FileHash -LiteralPath (Join-Path $dir $asset) -Algorithm SHA256).Hash.ToLower()
    $lines = @(
        (('0' * 64) + '  rtk-oss-x86_64-unknown-linux-musl.tar.gz'),
        "$hash  $asset"
    )
    [IO.File]::WriteAllLines((Join-Path $dir 'checksums.txt'), $lines)
    $dir
}

# Runs the installer against $Release (or a failing network) with RTK_VERSION
# set to $Version (unset if empty) and RTK_INSTALL_DIR to $InstallDir (default
# "$Name-bin"), after the PowerShell in $Prelude. Returns its exit code, output,
# install directory, the URLs it asked for and the temporary files it left.
function Invoke-Installer([string]$Name, [string]$Release, [string]$Version = 'fork-v0.0.0-test',
    [string]$InstallDir = '', [string]$Prelude = '', [switch]$FailDownload) {
    if (-not $InstallDir) { $InstallDir = Join-Path $work "$Name-bin" }
    $urls = Join-Path $work "$Name-urls.txt"
    $temp = Join-Path $work "$Name-temp"
    New-Item -ItemType Directory -Path $temp | Out-Null
    $download = if ($FailDownload) {
        "throw 'download failed (404)'"
    } else {
        "Copy-Item -LiteralPath (Join-Path '$Release' (Split-Path `$Url -Leaf)) -Destination `$OutFile"
    }
    $case = Join-Path $work "$Name.ps1"
    Set-Content -LiteralPath $case -Value @"
`$RtkInstallTestDefineOnly = `$true
. '$installer'
function Get-RtkDownload([string]`$Url, [string]`$OutFile) {
    Add-Content -LiteralPath '$urls' -Value `$Url
    $download
}
`$env:TMP = '$temp'; `$env:TEMP = '$temp'
$Prelude
Install-Rtk
"@
    if ($Version) { $env:RTK_VERSION = $Version } else { Remove-Item Env:RTK_VERSION -ErrorAction SilentlyContinue }
    $env:RTK_INSTALL_DIR = $InstallDir
    $exit = Invoke-Child $case $Name
    $dir = if ([IO.Path]::IsPathRooted($InstallDir)) { $InstallDir } else { $null }
    @{
        Exit = $exit
        Dir = $dir
        Urls = @(if (Test-Path -LiteralPath $urls) { Get-Content -LiteralPath $urls })
        Log = Get-Content -LiteralPath (Join-Path $work "$Name.log") -Raw
        Left = @(Get-ChildItem -LiteralPath $temp -Force) +
            @(if ($dir -and (Test-Path -LiteralPath $dir)) { Get-ChildItem -LiteralPath $dir -Filter '*.tmp' -Force })
    }
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

function Assert-Says([string]$Name, $Result, [string]$Text) {
    Assert ($Result.Log -and $Result.Log.Contains($Text)) "${Name}: output lacks '$Text' (see $work\$Name.log)"
}

function Assert-Clean([string]$Name, $Result) {
    Assert ($Result.Left.Count -eq 0) "${Name}: left $($Result.Left -join ', ')"
}

function Assert-Refused([string]$Name, $Result, [string]$Text) {
    Assert ($Result.Exit -ne 0) "${Name}: exit code $($Result.Exit), expected non-zero"
    Assert (-not (Test-Path -LiteralPath (Join-Path $Result.Dir 'rtk.exe'))) "${Name}: rtk.exe was installed"
    Assert-Clean $Name $Result
    if ($Text) { Assert-Says $Name $Result $Text }
}

function Assert-Installed([string]$Name, $Result, [string]$Dir) {
    Assert ($Result.Exit -eq 0) "${Name}: exit code $($Result.Exit), expected 0 (see $work\$Name.log)"
    $installed = Join-Path $Dir 'rtk.exe'
    Assert ((Test-Path -LiteralPath $installed) -and
        (Get-FileHash -LiteralPath $installed).Hash -eq (Get-FileHash -LiteralPath $program).Hash) "${Name}: rtk.exe not installed as packed in $Dir"
    Assert-Clean $Name $Result
}

try {
    # A good archive installs rtk.exe, byte for byte, from this repository's
    # releases only, and runs it once with --version.
    $good = New-Release 'good'
    $r = Invoke-Installer 'good' $good
    Assert-Installed 'good' $r $r.Dir
    Assert (-not (Test-Path -LiteralPath (Join-Path $r.Dir 'LICENSE'))) 'good: an entry other than rtk.exe was written'
    Assert-Says 'good' $r 'Verification: curl '
    Assert ($r.Urls.Count -eq 2) "good: $($r.Urls.Count) downloads, expected 2"
    foreach ($url in $r.Urls) {
        Assert ($url.StartsWith('https://github.com/yungSNAT/PascalsLetter/releases/download/fork-v0.0.0-test/')) "good: downloaded $url"
    }

    # Without RTK_VERSION, both files come from the latest release, with no API call.
    $r = Invoke-Installer 'latest' $good -Version ''
    Assert-Installed 'latest' $r $r.Dir
    Assert ($r.Urls.Count -eq 2) "latest: $($r.Urls.Count) downloads, expected 2"
    foreach ($url in $r.Urls) {
        Assert ($url.StartsWith('https://github.com/yungSNAT/PascalsLetter/releases/latest/download/')) "latest: downloaded $url"
    }

    # A relative RTK_INSTALL_DIR is taken from the current file-system folder,
    # also from another provider's location, where nothing is created.
    $cwd = Join-Path $work 'relative-cwd'
    New-Item -ItemType Directory -Path $cwd | Out-Null
    $r = Invoke-Installer 'relative' $good -InstallDir 'rel-bin' -Prelude "Set-Location -LiteralPath '$cwd'"
    Assert-Installed 'relative' $r (Join-Path $cwd 'rel-bin')
    Assert-Says 'relative' $r "Installed rtk.exe to $cwd\rel-bin\rtk.exe"
    $cwd = Join-Path $work 'registry-cwd'
    New-Item -ItemType Directory -Path $cwd | Out-Null
    $key = 'rtk-install-test-' + [guid]::NewGuid()
    $r = Invoke-Installer 'registry' $good -InstallDir $key -Prelude "Set-Location -LiteralPath '$cwd'; Set-Location HKCU:\"
    Assert-Installed 'registry' $r (Join-Path $cwd $key)
    if (Test-Path -LiteralPath "HKCU:\$key") {
        Assert $false "registry: HKCU:\$key was created (now removed)"
        Remove-Item -LiteralPath "HKCU:\$key" -Recurse
    }

    # A malformed tag is refused before any download.
    $r = Invoke-Installer 'tag' $good -Version 'not a tag'
    Assert-Refused 'tag' $r 'Not a release tag'
    Assert ($r.Urls.Count -eq 0) "tag: $($r.Urls.Count) downloads, expected none"

    # An architecture without a release build is refused before any download.
    $r = Invoke-Installer 'arch' $good -Prelude "`$env:PROCESSOR_ARCHITECTURE = 'x86'; Remove-Item Env:PROCESSOR_ARCHITEW6432 -ErrorAction SilentlyContinue"
    Assert-Refused 'arch' $r 'Unsupported architecture: x86'
    Assert ($r.Urls.Count -eq 0) "arch: $($r.Urls.Count) downloads, expected none"

    # An altered archive is refused.
    $tampered = New-Release 'tampered'
    [IO.File]::AppendAllText((Join-Path $tampered $asset), 'x')
    Assert-Refused 'tampered' (Invoke-Installer 'tampered' $tampered) 'checksum mismatch'

    # A partial download is refused.
    $partial = New-Release 'partial'
    $zip = Join-Path $partial $asset
    $bytes = [IO.File]::ReadAllBytes($zip)
    [IO.File]::WriteAllBytes($zip, $bytes[0..([int]($bytes.Length / 2))])
    Assert-Refused 'partial' (Invoke-Installer 'partial' $partial)

    # checksums.txt must hold exactly one line for the asset.
    $missing = New-Release 'missing'
    $sums = Join-Path $missing 'checksums.txt'
    [IO.File]::WriteAllLines($sums, @(Get-Content -LiteralPath $sums | Where-Object { -not $_.EndsWith($asset) }))
    Assert-Refused 'missing' (Invoke-Installer 'missing' $missing) "has 0 lines for $asset"
    $duplicate = New-Release 'duplicate'
    $sums = Join-Path $duplicate 'checksums.txt'
    [IO.File]::WriteAllLines($sums, @(Get-Content -LiteralPath $sums) + @(Get-Content -LiteralPath $sums)[-1])
    Assert-Refused 'duplicate' (Invoke-Installer 'duplicate' $duplicate) "has 2 lines for $asset"

    # An archive without rtk.exe at its root is refused.
    $nested = New-Release 'nested' -Entry 'bin\rtk.exe'
    Assert-Refused 'nested' (Invoke-Installer 'nested' $nested) 'rtk.exe not found at the root'

    # An HTTP failure is refused.
    Assert-Refused 'http' (Invoke-Installer 'http' $good -FailDownload) 'download failed'

    # A binary that does not start, or exits non-zero on --version (findstr
    # exits 2), fails the install, as in install.sh.
    $text = Join-Path $work 'not-a-program.txt'
    [IO.File]::WriteAllText($text, 'not a program')
    foreach ($case in @(@('norun', $text), @('badexit', (Join-Path $env:SystemRoot 'System32\findstr.exe')))) {
        $r = Invoke-Installer $case[0] (New-Release $case[0] -Program $case[1])
        Assert ($r.Exit -ne 0) "$($case[0]): exit code $($r.Exit), expected non-zero"
        Assert-Says $case[0] $r 'does not run on this system'
        Assert-Clean $case[0] $r
    }

    # An rtk.exe in use is left in place, and the user is told why.
    $busyDir = Join-Path $work 'busy-bin'
    New-Item -ItemType Directory -Path $busyDir | Out-Null
    $old = Join-Path $busyDir 'rtk.exe'
    [IO.File]::WriteAllText($old, 'old rtk')
    $lock = [IO.File]::Open($old, 'Open', 'Read', 'None')
    try { $r = Invoke-Installer 'busy' $good } finally { $lock.Dispose() }
    Assert ($r.Exit -ne 0) "busy: exit code $($r.Exit), expected non-zero"
    Assert ([IO.File]::ReadAllText($old) -eq 'old rtk') 'busy: the existing rtk.exe was changed'
    Assert-Says 'busy' $r 'is in use; close it and run again'
    Assert-Clean 'busy' $r

    # The real download refuses a file that reached it through a redirect to
    # plain HTTP, in the response shapes of Windows PowerShell 5.1 and PowerShell 7.
    $redirect = Join-Path $work 'redirect.ps1'
    Set-Content -LiteralPath $redirect -Value @"
`$RtkInstallTestDefineOnly = `$true
. '$installer'
Set-StrictMode -Version Latest
function Invoke-WebRequest([string]`$Uri, [string]`$OutFile, [switch]`$UseBasicParsing, [switch]`$PassThru) {
    Set-Content -LiteralPath `$OutFile -Value 'body'
    `$final = [uri]`$script:final
    if (`$script:shape -eq '5.1') { [pscustomobject]@{ BaseResponse = [pscustomobject]@{ ResponseUri = `$final } } }
    else { [pscustomobject]@{ BaseResponse = [pscustomobject]@{ RequestMessage = [pscustomobject]@{ RequestUri = `$final } } } }
}
foreach (`$script:shape in '5.1', '7') {
    foreach (`$script:final in 'https://objects.example/x', 'http://objects.example/x') {
        try { Get-RtkDownload 'https://github.com/x' '$work\redirect.out'; "redirect: `$shape `$final ok" }
        catch { "redirect: `$shape `$final threw `$(`$_.Exception.Message)" }
    }
}
"@
    $null = Invoke-Child $redirect 'redirect'
    $r = @{ Log = Get-Content -LiteralPath (Join-Path $work 'redirect.log') -Raw }
    foreach ($shape in '5.1', '7') {
        Assert-Says 'redirect' $r "redirect: $shape https://objects.example/x ok"
        Assert-Says 'redirect' $r "redirect: $shape http://objects.example/x threw https://github.com/x was redirected to http://objects.example/x, which is not HTTPS"
    }

    # Through `irm | iex` in a dot-sourced script, the installer runs (here it
    # stops at the tag) and leaves the caller's variables and TLS setting alone.
    $iex = Join-Path $work 'iex.ps1'
    Set-Content -LiteralPath $iex -Value @"
`$repo = 'caller'; `$asset = 'caller'; `$base = 'caller'
`$tls = [Net.ServicePointManager]::SecurityProtocol
try { . { Get-Content -LiteralPath '$installer' -Raw | Invoke-Expression }; 'iex: returned' } catch { "iex: threw `$(`$_.Exception.Message)" }
"iex: `$repo `$asset `$base `$([Net.ServicePointManager]::SecurityProtocol -eq `$tls)"
"@
    $env:RTK_VERSION = 'not a tag'
    $env:RTK_INSTALL_DIR = Join-Path $work 'iex-bin'
    $null = Invoke-Child $iex 'iex'
    $log = Get-Content -LiteralPath (Join-Path $work 'iex.log') -Raw
    Assert ($log.Contains("iex: threw Not a release tag")) "iex: installer did not run (see $work\iex.log)"
    Assert ($log.Contains('iex: caller caller caller True')) "iex: the caller's session was changed (see $work\iex.log)"

    # Run as a file, a failure exits 1.
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
Write-Host "install.ps1: all 18 cases passed ($($PSVersionTable.PSEdition) $($PSVersionTable.PSVersion))" -ForegroundColor Green
