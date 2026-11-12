# PascalsLetter installer for Windows - https://github.com/yungSNAT/PascalsLetter
# Usage: irm https://raw.githubusercontent.com/yungSNAT/PascalsLetter/develop/install.ps1 | iex
#
# Installs rtk.exe from the latest PascalsLetter release for the current user,
# without elevation, after checking the archive's SHA-256 against the
# release's checksums.txt. It never runs what it downloads and never changes
# PATH. Works in Windows PowerShell 5.1 and PowerShell 7.
#
# RTK_VERSION      pin a release tag, such as fork-v1.2.0 (default: latest)
# RTK_INSTALL_DIR  install directory (default: $HOME\.local\bin, as install.sh)

$Repo = 'yungSNAT/PascalsLetter'
$BinaryName = 'rtk.exe'
# PascalsLetter releases only the Apache-2.0 edition (`rtk-oss`); the binary
# it installs is still `rtk`, so agent hooks keep working.
$AssetName = 'rtk-oss-x86_64-pc-windows-msvc.zip'

function Write-Info([string]$Message) { Write-Host "[INFO] $Message" -ForegroundColor Green }
function Write-Warn([string]$Message) { Write-Host "[WARN] $Message" -ForegroundColor Yellow }

# Downloads $Url to $OutFile, or throws. Kept separate so the tests can stand
# in for the network.
function Get-RtkDownload([string]$Url, [string]$OutFile) {
    Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
}

function Get-RtkVersion {
    if ($env:RTK_VERSION) {
        $version = $env:RTK_VERSION
        Write-Info "Using pinned version from RTK_VERSION: $version"
    } else {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
        $version = [string]$release.tag_name
    }
    # The tag goes into a URL: accept only what a release tag can be.
    if ($version -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$') {
        throw "Not a release tag: '$version' (set RTK_VERSION=fork-vX.Y.Z to pin)"
    }
    $version
}

# The SHA-256 that checksums.txt gives for $Name, or throws.
function Get-ExpectedHash([string]$ChecksumsFile, [string]$Name) {
    $hashes = @(Get-Content -LiteralPath $ChecksumsFile | ForEach-Object {
        if ($_ -match '^([0-9A-Fa-f]{64}) [ *](.+)$' -and $Matches[2] -eq $Name) { $Matches[1] }
    })
    if ($hashes.Count -ne 1) {
        throw "checksum for $Name not found in checksums.txt - refusing to install"
    }
    $hashes[0]
}

function Install-Rtk {
    # Set here, not at the top: through `irm | iex` the script runs in the
    # caller's session, whose preferences it must leave alone.
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue' # the progress bar slows 5.1's downloads
    Set-StrictMode -Version Latest

    Write-Info 'Installing rtk-oss...'
    # A 32-bit PowerShell on 64-bit Windows reports x86 here and the real one below.
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    if ($arch -ne 'AMD64' -and $arch -ne 'ARM64') {
        throw "Unsupported architecture: $arch (releases hold an x86_64 build only)"
    }
    # Windows PowerShell 5.1 may not offer TLS 1.2 by default.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $version = Get-RtkVersion
    Write-Info "Version: $version"
    $installDir = if ($env:RTK_INSTALL_DIR) { $env:RTK_INSTALL_DIR } else { Join-Path $HOME '.local\bin' }
    $base = "https://github.com/$Repo/releases/download/$version"

    $tempDir = Join-Path ([IO.Path]::GetTempPath()) ("rtk-install-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Path $tempDir | Out-Null
    try {
        $checksums = Join-Path $tempDir 'checksums.txt'
        $archive = Join-Path $tempDir $AssetName
        Write-Info 'Downloading checksums...'
        Get-RtkDownload "$base/checksums.txt" $checksums
        $expected = Get-ExpectedHash $checksums $AssetName

        Write-Info "Downloading from: $base/$AssetName"
        Get-RtkDownload "$base/$AssetName" $archive

        # A partial or altered download fails here, before anything is unpacked.
        Write-Info 'Verifying SHA-256 checksum...'
        $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
        if ($actual -ne $expected) {
            throw "checksum mismatch! expected=$($expected.ToLower()) actual=$($actual.ToLower()) - refusing to install"
        }
        Write-Info 'Checksum verified.'

        # Take only the binary at the archive's root, so no other entry is written.
        Write-Info 'Extracting...'
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $staged = Join-Path $tempDir $BinaryName
        $zip = [IO.Compression.ZipFile]::OpenRead($archive)
        try {
            $entry = @($zip.Entries | Where-Object { $_.FullName -eq $BinaryName })
            if ($entry.Count -ne 1) { throw "$BinaryName not found in $AssetName - refusing to install" }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry[0], $staged)
        } finally {
            $zip.Dispose()
        }

        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
        $target = Join-Path $installDir $BinaryName
        $replacing = Test-Path -LiteralPath $target
        Move-Item -LiteralPath $staged -Destination $target -Force
        if ($replacing) { Write-Info "Replaced the existing $target" }
        Write-Info "Successfully installed $BinaryName ($version) to $target"
    } finally {
        Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }

    $onPath = @($env:PATH -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -eq $installDir.TrimEnd('\')) })
    if ($onPath.Count -eq 0) {
        Write-Warn "$installDir is not in PATH. PATH was not changed; to add it for your user, run:"
        Write-Warn "  [Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path', 'User') + ';$installDir', 'User')"
    }
    Write-Host ''
    Write-Info "Installation complete! Open a new terminal and run 'rtk --help' to get started."
}

# Dot-sourcing (the tests) defines the functions only. Run as a file, a
# failure exits 1; through `irm | iex` it throws, which leaves the caller's
# session open.
if ($MyInvocation.InvocationName -ne '.') {
    if ($PSCommandPath) {
        try { Install-Rtk } catch { Write-Host "[ERROR] $($_.Exception.Message)" -ForegroundColor Red; exit 1 }
    } else {
        Install-Rtk
    }
}
