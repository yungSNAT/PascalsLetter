# PascalsLetter installer for Windows - https://github.com/yungSNAT/PascalsLetter
# Usage: irm https://raw.githubusercontent.com/yungSNAT/PascalsLetter/develop/install.ps1 | iex
#
# Installs rtk.exe from the latest PascalsLetter release for the current user,
# without elevation, after checking the archive's SHA-256 against the
# release's checksums.txt. It never changes PATH, and runs only the verified
# rtk.exe, once, with --version. Works in Windows PowerShell 5.1 and PowerShell 7.
#
# RTK_VERSION      pin a release tag, such as fork-v1.2.0 (default: latest)
# RTK_INSTALL_DIR  install directory (default: $HOME\.local\bin, as install.sh)

# Downloads $Url to $OutFile over HTTPS only, as install.sh does, or throws.
# Kept separate so the tests can stand in for the network.
function Get-RtkDownload([string]$Url, [string]$OutFile) {
    # curl.exe's errors go to the console; under 'Stop', Windows PowerShell 5.1
    # would turn them into an exception and hide the exit code.
    $ErrorActionPreference = 'Continue'
    if (-not (Get-Command curl.exe -CommandType Application -ErrorAction SilentlyContinue)) {
        throw 'curl.exe not found; Windows 10 1803 and later include it'
    }
    curl.exe --proto '=https' --tlsv1.2 -fsSL -o $OutFile $Url
    if ($LASTEXITCODE -ne 0) { throw "download failed (curl exit $LASTEXITCODE): $Url" }
}

function Install-Rtk {
    # Everything is local to this function: through `irm | iex` the script
    # runs in the caller's session, which it must leave as it was.
    $ErrorActionPreference = 'Stop'
    Set-StrictMode -Version Latest
    $repo = 'yungSNAT/PascalsLetter'
    # PascalsLetter releases only the Apache-2.0 edition (`rtk-oss`); the binary
    # it installs is still `rtk`, so agent hooks keep working.
    $asset = 'rtk-oss-x86_64-pc-windows-msvc.zip'
    function Write-Info([string]$Message) { Write-Host "[INFO] $Message" -ForegroundColor Green }

    Write-Info 'Installing rtk-oss...'
    # A 32-bit PowerShell on 64-bit Windows reports x86 here and the real one below.
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    if ($arch -ne 'AMD64' -and $arch -ne 'ARM64') {
        throw "Unsupported architecture: $arch (releases hold an x86_64 build only)"
    }

    if ($env:RTK_VERSION) {
        # The tag goes into a URL: accept only what a release tag can be.
        if ($env:RTK_VERSION -notmatch '\A[A-Za-z0-9][A-Za-z0-9._-]*\z') {
            throw "Not a release tag: '$env:RTK_VERSION' (set RTK_VERSION=fork-vX.Y.Z to pin)"
        }
        Write-Info "Using pinned version from RTK_VERSION: $env:RTK_VERSION"
        $base = "https://github.com/$repo/releases/download/$env:RTK_VERSION"
    } else {
        # A release published between the 2 downloads fails the checksum.
        $base = "https://github.com/$repo/releases/latest/download"
    }
    $installDir = if ($env:RTK_INSTALL_DIR) { $env:RTK_INSTALL_DIR } else { Join-Path $HOME '.local\bin' }
    $installDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($installDir)
    $target = Join-Path $installDir 'rtk.exe'

    $tempDir = Join-Path ([IO.Path]::GetTempPath()) ("rtk-install-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Path $tempDir | Out-Null
    try {
        $checksums = Join-Path $tempDir 'checksums.txt'
        $archive = Join-Path $tempDir $asset
        Write-Info 'Downloading checksums...'
        Get-RtkDownload "$base/checksums.txt" $checksums
        $expected = @(Get-Content -LiteralPath $checksums | ForEach-Object {
            if ($_ -match '\A([0-9A-Fa-f]{64}) [ *](.+)\z' -and $Matches[2] -eq $asset) { $Matches[1] }
        })
        if ($expected.Count -ne 1) {
            throw "checksums.txt has $($expected.Count) lines for $asset, expected 1 - refusing to install"
        }

        Write-Info "Downloading from: $base/$asset"
        Get-RtkDownload "$base/$asset" $archive

        # A partial or altered download fails here, before anything is unpacked.
        Write-Info 'Verifying SHA-256 checksum...'
        $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
        if ($actual -ne $expected[0]) {
            throw "checksum mismatch! expected=$($expected[0].ToLower()) actual=$($actual.ToLower()) - refusing to install"
        }
        Write-Info 'Checksum verified.'

        # Take only the binary at the archive's root, so no other entry is written.
        Write-Info 'Extracting...'
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $staged = Join-Path $tempDir 'rtk.exe'
        $zip = [IO.Compression.ZipFile]::OpenRead($archive)
        try {
            $entry = @($zip.Entries | Where-Object { $_.FullName -eq 'rtk.exe' })
            if ($entry.Count -ne 1) { throw "rtk.exe not found at the root of $asset - refusing to install" }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry[0], $staged)
        } finally {
            $zip.Dispose()
        }

        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
        $replacing = Test-Path -LiteralPath $target
        try {
            Move-Item -LiteralPath $staged -Destination $target -Force
        } catch {
            if ($replacing) { throw "$target is in use; close it and run again ($($_.Exception.Message))" }
            throw
        }
        if ($replacing) { Write-Info "Replaced the existing $target" }
        Write-Info "Installed rtk.exe to $target"
    } finally {
        Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }

    # As install.sh does: the installed binary must run here.
    try {
        $version = (& $target --version) -join ' '
        $runs = $LASTEXITCODE -eq 0
    } catch {
        $version = $_.Exception.GetBaseException().Message
        $runs = $false
    }
    if (-not $runs) {
        throw "$target was installed but does not run on this system ($version). On Windows on Arm, x64 programs need Windows 11; antivirus can also block a new program."
    }
    Write-Info "Verification: $version"

    $onPath = @($env:PATH -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -eq $installDir.TrimEnd('\')) })
    if ($onPath.Count -eq 0) {
        Write-Host "[WARN] $installDir is not on PATH, which this script leaves unchanged. Add it to your user PATH to run rtk from any terminal." -ForegroundColor Yellow
    }
    Write-Host ''
    Write-Info "Installation complete! Open a new terminal and run 'rtk --help' to get started."
}

# The tests set this to define the functions without installing.
if (-not (Get-Variable -Name RtkInstallTestDefineOnly -ValueOnly -ErrorAction Ignore)) {
    Install-Rtk
}
