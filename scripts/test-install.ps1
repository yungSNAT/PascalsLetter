# Tests for install.ps1. Run with each PowerShell the installer supports:
#   powershell -NoProfile -File scripts/test-install.ps1   # Windows PowerShell 5.1
#   pwsh -NoProfile -File scripts/test-install.ps1         # PowerShell 7
#
# Each case runs the installer in a child of the same PowerShell, with the
# network replaced by a fixture release, and checks its exit code, its message,
# whether rtk.exe was installed and that its temporary files are gone. The
# 'download' case runs the real download against local servers instead. Each
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
function Invoke-Child([string]$Script, [string]$Name, [string[]]$Arguments = @()) {
    $ErrorActionPreference = 'Continue'
    & $shell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $Script @Arguments *> (Join-Path $work "$Name.log")
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

    # An existing rtk.exe not in use is replaced by the verified one.
    $replaceDir = Join-Path $work 'replace-bin'
    New-Item -ItemType Directory -Path $replaceDir | Out-Null
    [IO.File]::WriteAllText((Join-Path $replaceDir 'rtk.exe'), 'old rtk')
    $r = Invoke-Installer 'replace' $good
    Assert-Installed 'replace' $r $replaceDir
    Assert-Says 'replace' $r 'Replaced the existing'

    # The real download, against local HTTPS and HTTP servers on the loopback
    # address, follows redirects between HTTPS addresses only, and refuses one to
    # HTTP before sending it any request, also when HTTP would lead back to HTTPS.
    # The server's certificate is made for this test and trusted only by the
    # child that downloads; no certificate store is touched.
    $server = Join-Path $work 'server.ps1'
    Set-Content -LiteralPath $server -Value @'
param([string]$Dir)
$ErrorActionPreference = 'Stop'
$rsa = [Security.Cryptography.RSACng]::new(2048)
$csr = [Security.Cryptography.X509Certificates.CertificateRequest]::new('CN=127.0.0.1', $rsa,
    [Security.Cryptography.HashAlgorithmName]::SHA256, [Security.Cryptography.RSASignaturePadding]::Pkcs1)
$now = [DateTimeOffset]::UtcNow
# Schannel needs the key in a key container: this import makes a temporary one,
# which Dispose deletes.
$cert = [Security.Cryptography.X509Certificates.X509Certificate2]::new(
    $csr.CreateSelfSigned($now.AddMinutes(-5), $now.AddHours(1)).Export('Pfx'), [string]$null, 'UserKeySet')
$tls = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
$plain = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
$tls.Start(); $plain.Start()
$s = "https://127.0.0.1:$($tls.LocalEndpoint.Port)"
$h = "http://127.0.0.1:$($plain.LocalEndpoint.Port)"
$redirects = @{
    '/to-http' = "$h/plain"; '/chain' = "$h/back"; '/back' = "$s/file"
    '/hop' = "$s/hop2"; '/hop2' = '/file'; '/loop' = '/loop'
}
[IO.File]::WriteAllText("$Dir\ready.tmp", "$s $h $($cert.Thumbprint)")
Move-Item -LiteralPath "$Dir\ready.tmp" -Destination "$Dir\ready"
$deadline = [DateTime]::UtcNow.AddMinutes(3)
try {
    while (-not (Test-Path -LiteralPath "$Dir\stop") -and [DateTime]::UtcNow -lt $deadline) {
        foreach ($listener in $tls, $plain) {
            if (-not $listener.Pending()) { continue }
            $client = $listener.AcceptTcpClient()
            try {
                $stream = $client.GetStream()
                $scheme = 'http'
                if ($listener -eq $tls) {
                    $stream = [Net.Security.SslStream]::new($stream)
                    $stream.AuthenticateAsServer($cert, $false, 'Tls12', $false)
                    $scheme = 'https'
                }
                $reader = [IO.StreamReader]::new($stream)
                $path = ($reader.ReadLine() -split ' ')[1]
                while ($reader.ReadLine()) {}
                Add-Content -LiteralPath "$Dir\requests.log" -Value "$scheme $path"
                $head, $body = if ($redirects.ContainsKey($path)) { "302 Found`r`nLocation: $($redirects[$path])", '' }
                    elseif ($path -eq '/file') { '200 OK', 'release body' }
                    elseif ($scheme -eq 'http') { '200 OK', 'plain http body' }
                    else { '404 Not Found', '' }
                $bytes = [Text.Encoding]::ASCII.GetBytes("HTTP/1.1 $head`r`nContent-Length: $($body.Length)`r`nConnection: close`r`n`r`n$body")
                $stream.Write($bytes, 0, $bytes.Length)
                $stream.Flush()
            } catch {
                Add-Content -LiteralPath "$Dir\server-errors.log" -Value "$_"
            } finally {
                $client.Close()
            }
        }
        Start-Sleep -Milliseconds 20
    }
} finally {
    $tls.Stop(); $plain.Stop(); $cert.Dispose()
}
'@
    $download = Join-Path $work 'download.ps1'
    Set-Content -LiteralPath $download -Value @'
param([string]$Installer, [string]$Dir, [string]$Server, [string]$Thumbprint)
$RtkInstallTestDefineOnly = $true
. $Installer
Set-StrictMode -Version Latest
# A scriptblock has no runspace on the thread that checks the certificate, so
# the check is compiled.
Add-Type -TypeDefinition @"
public static class RtkTestTrust {
    public static string Thumbprint;
    public static bool Check(object sender, System.Security.Cryptography.X509Certificates.X509Certificate certificate,
        System.Security.Cryptography.X509Certificates.X509Chain chain, System.Net.Security.SslPolicyErrors errors) {
        return certificate != null && certificate.GetCertHashString() == Thumbprint;
    }
}
"@
[RtkTestTrust]::Thumbprint = $Thumbprint
[Net.ServicePointManager]::ServerCertificateValidationCallback =
    [Delegate]::CreateDelegate([Net.Security.RemoteCertificateValidationCallback], [RtkTestTrust].GetMethod('Check'))
foreach ($case in 'hop', 'to-http', 'chain', 'loop', 'missing') {
    $out = Join-Path $Dir "download-$case.out"
    try { Get-RtkDownload "$Server/$case" $out; "download: $case ok $(Get-Content -LiteralPath $out -Raw)" }
    catch { "download: $case threw $($_.Exception.Message)" }
    "download: $case file $(Test-Path -LiteralPath $out)"
}
'@
    $serverProcess = Start-Process -FilePath $shell -PassThru -NoNewWindow `
        -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', "`"$server`"", "`"$work`"") `
        -RedirectStandardOutput (Join-Path $work 'server.out') -RedirectStandardError (Join-Path $work 'server.err')
    try {
        $ready = Join-Path $work 'ready'
        $wait = [Diagnostics.Stopwatch]::StartNew()
        while (-not (Test-Path -LiteralPath $ready) -and -not $serverProcess.HasExited -and $wait.Elapsed.TotalSeconds -lt 60) {
            Start-Sleep -Milliseconds 100
        }
        if (-not (Test-Path -LiteralPath $ready)) { throw "the test server did not start (see $work\server.err)" }
        $s, $h, $thumbprint = (Get-Content -LiteralPath $ready -Raw).Split(' ')
        $null = Invoke-Child $download 'download' @($installer, $work, $s, $thumbprint)
    } finally {
        New-Item -ItemType File -Path (Join-Path $work 'stop') | Out-Null
        if (-not $serverProcess.WaitForExit(10000)) { $serverProcess.Kill() }
    }
    $r = @{ Log = Get-Content -LiteralPath (Join-Path $work 'download.log') -Raw }
    $requests = @(if (Test-Path -LiteralPath (Join-Path $work 'requests.log')) { Get-Content -LiteralPath (Join-Path $work 'requests.log') })
    Assert-Says 'download' $r 'download: hop ok release body'
    Assert-Says 'download' $r "download: to-http threw $s/to-http was redirected to $h/plain, which is not HTTPS"
    Assert-Says 'download' $r "download: chain threw $s/chain was redirected to $h/back, which is not HTTPS"
    Assert-Says 'download' $r "download: loop threw $s/loop was redirected more than 10 times"
    Assert-Says 'download' $r 'download: missing threw'
    foreach ($case in 'to-http', 'chain', 'loop', 'missing') { Assert-Says 'download' $r "download: $case file False" }
    $sent = @($requests | Where-Object { $_ -like 'http *' })
    Assert ($sent.Count -eq 0) "download: plain HTTP requests reached the server: $($sent -join ', ')"
    Assert (($requests -join ',') -eq 'https /hop,https /hop2,https /file,https /to-http,https /chain,' + ((1..11 | ForEach-Object { 'https /loop' }) -join ',') + ',https /missing') "download: requests were $($requests -join ', ')"
    Assert (-not (Test-Path -LiteralPath (Join-Path $work 'server-errors.log'))) "download: the test server failed (see $work\server-errors.log)"

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
Write-Host "install.ps1: all 19 cases passed ($($PSVersionTable.PSEdition) $($PSVersionTable.PSVersion))" -ForegroundColor Green
# The last case leaves its child's exit code, 1, in $LASTEXITCODE, and a caller that
# dot-sources this script, as a GitHub Actions step does, would exit with it.
exit 0
