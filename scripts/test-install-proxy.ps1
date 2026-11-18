# Installs the latest published release with install.ps1 behind 2 local
# mitmproxy instances set as the Windows system proxy: one that inspects TLS
# with its own certificate authority, and one that also asks for a user name
# and password. Each case runs the installer in a child of this PowerShell and
# checks, from the proxy's log, that the download really went through it.
#
# It changes this user's proxy settings and the machine's trusted root
# certificates, restoring both at the end, so it is for a disposable machine
# such as a CI runner, run as administrator. $env:MITMDUMP names mitmdump.exe:
#   powershell -NoProfile -File scripts/test-install-proxy.ps1   # Windows PowerShell 5.1
#   pwsh -NoProfile -File scripts/test-install-proxy.ps1         # PowerShell 7

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not $env:MITMDUMP -or -not (Test-Path -LiteralPath $env:MITMDUMP)) { throw 'Set MITMDUMP to the path of mitmdump.exe' }
$installer = (Resolve-Path (Join-Path $PSScriptRoot '..\install.ps1')).Path
$shell = (Get-Process -Id $PID).Path
$work = Join-Path ([IO.Path]::GetTempPath()) ("rtk-install-proxy-" + [guid]::NewGuid())
$conf = Join-Path $work 'mitmproxy'
New-Item -ItemType Directory -Path $conf | Out-Null
$settings = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
$failures = @()

function Assert([bool]$Condition, [string]$Message) {
    if (-not $Condition) { $script:failures += $Message; Write-Host "FAIL: $Message" -ForegroundColor Red }
}

# A free loopback port for a proxy to listen on.
function Get-FreePort {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try { $listener.LocalEndpoint.Port } finally { $listener.Stop() }
}

# Starts mitmdump on $Port with $Arguments, logging to $Name-proxy.log, and
# waits until it accepts connections.
function Start-Proxy([string]$Name, [int]$Port, [string[]]$Arguments = @()) {
    $log = Join-Path $work "$Name-proxy.log"
    $all = @('--listen-host', '127.0.0.1', '--listen-port', $Port, '--set', "confdir=$conf", '--flow-detail', '1') + $Arguments
    $process = Start-Process -FilePath $env:MITMDUMP -ArgumentList $all -PassThru -NoNewWindow `
        -RedirectStandardOutput $log -RedirectStandardError "$log.err"
    for ($i = 0; $i -lt 60; $i++) {
        if ($process.HasExited) { throw "mitmdump for $Name exited: $(Get-Content -LiteralPath "$log.err" -Raw)" }
        $client = [Net.Sockets.TcpClient]::new()
        try { $client.Connect('127.0.0.1', $Port); return $process } catch { Start-Sleep -Milliseconds 500 } finally { $client.Dispose() }
    }
    throw "mitmdump for $Name did not listen on port $Port"
}

# Makes 127.0.0.1:$Port this user's system proxy, as Internet Options would.
function Set-SystemProxy([int]$Port) {
    Set-ItemProperty -Path $settings -Name ProxyServer -Value "127.0.0.1:$Port"
    Set-ItemProperty -Path $settings -Name ProxyEnable -Value 1 -Type DWord
}

# Runs the installer in a child PowerShell after $Prelude, logging to $Name.log,
# and returns its exit code, its output, the installed rtk.exe's --version and
# the proxy's log.
function Invoke-Installer([string]$Name, [string]$Proxy, [string]$Prelude = '') {
    $ErrorActionPreference = 'Continue'
    $dir = Join-Path $work "$Name-bin"
    $env:RTK_INSTALL_DIR = $dir
    $command = "Get-Content -LiteralPath '$installer' -Raw | Invoke-Expression"
    if ($Prelude) { $command = "$Prelude; $command" }
    & $shell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command $command *> (Join-Path $work "$Name.log")
    $exit = $LASTEXITCODE
    $exe = Join-Path $dir 'rtk.exe'
    $version = if (Test-Path -LiteralPath $exe) { (& $exe --version) -join ' ' } else { '' }
    @{
        Exit = $exit
        Log = Get-Content -LiteralPath (Join-Path $work "$Name.log") -Raw
        Version = $version
        Proxy = Get-Content -LiteralPath (Join-Path $work "$Proxy-proxy.log") -Raw
    }
}

function Assert-Installed([string]$Name, $Result) {
    Assert ($Result.Exit -eq 0 -and $Result.Version -like 'rtk *') "${Name}: exit $($Result.Exit), rtk.exe '$($Result.Version)' (see $work\$Name.log)"
}

function Assert-Refused([string]$Name, $Result, [string]$Pattern) {
    Assert ($Result.Exit -ne 0 -and -not $Result.Version) "${Name}: exit $($Result.Exit), rtk.exe '$($Result.Version)', expected a refusal"
    Assert ($Result.Log -match $Pattern) "${Name}: output does not match '$Pattern' (see $work\$Name.log)"
}

# The proxy logs each request it decrypts as 'GET <url>', shortened to its
# console width; a tunnel it did not open would show only CONNECT.
function Assert-Decrypted([string]$Name, $Result, [string]$Proxy) {
    foreach ($request in 'GET https://github.com/yungSNAT/PascalsLetter/releases/', 'GET https://release-assets.githubusercontent.com/') {
        Assert ($Result.Proxy -and $Result.Proxy.Contains($request)) "${Name}: the proxy did not decrypt '$request' (see $work\$Proxy-proxy.log)"
    }
}

$saved = Get-ItemProperty -Path $settings
$proxies = @()
$ca = $null
# mitmdump's output is a file, which Python would otherwise buffer.
$env:PYTHONUNBUFFERED = '1'
try {
    $inspectPort = Get-FreePort
    $proxies += Start-Proxy 'inspect' $inspectPort
    $authPort = Get-FreePort
    $proxies += Start-Proxy 'auth' $authPort @('--proxyauth', 'rtk:proxy-test')
    $caFile = Join-Path $conf 'mitmproxy-ca-cert.cer'
    Assert (Test-Path -LiteralPath $caFile) 'mitmdump wrote no CA certificate'

    # Until its CA is trusted, the inspecting proxy's certificate is refused,
    # which shows the download goes through it.
    Set-SystemProxy $inspectPort
    $r = Invoke-Installer 'untrusted' 'inspect'
    Assert-Refused 'untrusted' $r 'SSL|TLS|trust'
    Assert ($r.Proxy -match 'github\.com') "untrusted: the proxy saw no request to github.com (see $work\inspect-proxy.log)"

    $ca = Import-Certificate -FilePath $caFile -CertStoreLocation Cert:\LocalMachine\Root
    $r = Invoke-Installer 'inspected' 'inspect'
    Assert-Installed 'inspected' $r
    Assert-Decrypted 'inspected' $r 'inspect'

    # The authenticating proxy refuses a download without its credentials, and
    # passes one with the credentials set on the session's default proxy.
    Set-SystemProxy $authPort
    $r = Invoke-Installer 'no-credentials' 'auth'
    Assert-Refused 'no-credentials' $r '407'
    $r = Invoke-Installer 'credentials' 'auth' "[Net.WebRequest]::DefaultWebProxy.Credentials = [Net.NetworkCredential]::new('rtk', 'proxy-test')"
    Assert-Installed 'credentials' $r
    Assert-Decrypted 'credentials' $r 'auth'
} finally {
    foreach ($name in 'ProxyServer', 'ProxyEnable') {
        if ($saved.PSObject.Properties[$name]) {
            Set-ItemProperty -Path $settings -Name $name -Value $saved.$name
        } else {
            Remove-ItemProperty -Path $settings -Name $name -ErrorAction SilentlyContinue
        }
    }
    if ($ca) { Remove-Item -LiteralPath "Cert:\LocalMachine\Root\$($ca.Thumbprint)" }
    # Their logs stay open until they have exited.
    foreach ($process in $proxies) {
        Stop-Process -InputObject $process -Force -ErrorAction SilentlyContinue
        $null = $process.WaitForExit(10000)
    }
    Remove-Item Env:RTK_INSTALL_DIR -ErrorAction SilentlyContinue
}

if ($failures.Count -gt 0) {
    foreach ($log in Get-ChildItem -LiteralPath $work -Filter '*.log') {
        Write-Host "--- $($log.Name)"
        Get-Content -LiteralPath $log.FullName -Tail 40
    }
    Write-Host "$($failures.Count) failure(s); logs in $work" -ForegroundColor Red
    exit 1
}
Remove-Item -LiteralPath $work -Recurse -Force
Write-Host "install.ps1: installed through an inspecting and an authenticating proxy ($($PSVersionTable.PSEdition) $($PSVersionTable.PSVersion))" -ForegroundColor Green
exit 0
