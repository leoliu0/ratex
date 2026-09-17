<#
.SYNOPSIS
    Installer for the tex-suite Rust TeX engine collection on Windows.

.DESCRIPTION
    Self-contained installer for pdflatex, xelatex, lualatex, bibtex and
    texmk/latexmk. Copies the release binaries into <root>\bin and the texmf
    TDS tree into <root>\share\tex-suite,
    then persistently registers <root>\bin in PATH and sets TEXMFLOCAL.

    Default install root (per-user):  %LOCALAPPDATA%\tex-suite
    With -System (elevated):          %ProgramFiles%\tex-suite

    Works in Windows PowerShell 5.1 and PowerShell 7+.

.PARAMETER Uninstall
    Remove installed files, the bin entry from PATH, and the tex-suite
    environment variables (TEXMFLOCAL, TEX_SUITE_DATA).

.PARAMETER System
    Install/uninstall machine-wide under %ProgramFiles%\tex-suite using the
    Machine environment. Requires an elevated (Administrator) session.

.PARAMETER SourceDir
    Directory holding the staged bundle: SourceDir\bin\*.exe and
    SourceDir\share\tex-suite\texmf. By default the script
    looks next to itself (bundle layout), then for a Cargo checkout
    (target\release; the default format is embedded).

.PARAMETER InstallDir
    Override the install root entirely.

.PARAMETER Help
    Show usage and exit.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File .\install-windows.ps1

.EXAMPLE
    .\install-windows.ps1 -System

.EXAMPLE
    .\install-windows.ps1 -Uninstall
#>

#Requires -Version 5.1
[CmdletBinding()]
param(
    [switch]$Uninstall,
    [switch]$System,
    [switch]$Help,
    [switch]$AliasLatexmk,
    [switch]$NoAliasLatexmk,
    [switch]$ReplaceLatexmk,
    [switch]$NoReplaceLatexmk,
    [string]$SourceDir = '',
    [Alias('Prefix')]
    [string]$InstallDir = '',
    [switch]$NoPath,
    [switch]$SkipVerify
)
$ErrorActionPreference = 'Stop'

# Allow "run by pasting into a console" where $PSScriptRoot is not defined.
if (-not $PSScriptRoot) {
    $PSScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
}

# --------------------------------------------------------------- constants

$Script:CoreExes = @('ratex.exe')
$Script:ExtraExes = @()
$Script:Shims = @()
if ($ReplaceLatexmk) { $AliasLatexmk = $true }
if ($NoReplaceLatexmk) { $NoAliasLatexmk = $true }

if (-not $Uninstall) {
    if (-not $AliasLatexmk -and -not $NoAliasLatexmk) {
        if ([Environment]::UserInteractive) {
            $ans = Read-Host 'Install "latexmk" alias pointing to texmk? (recommended for TeXstudio/VS Code) [Y/n]'
            if ($ans -match '^[nN]') {
                $NoAliasLatexmk = $true
            } else {
                $AliasLatexmk = $true
            }
        } else {
            $AliasLatexmk = $true
        }
    }
}

if ($NoAliasLatexmk) {
    $Script:Shims = @($Script:Shims | Where-Object { $_.Name -ne 'latexmk.exe' })
}
$Script:AllInstalledExes = $Script:CoreExes + $Script:ExtraExes + ($Script:Shims | ForEach-Object { $_.Name })
$Script:EnvVars = @('TEXMFLOCAL', 'TEX_SUITE_DATA')
$Script:InstallManifestName = '.tex-suite-install.json'
$Script:InstallManifestSchema = 'tex-suite-install-v2'

# ---------------------------------------------------------------- helpers

function Write-Info { param([string]$Message) Write-Host "[tex-suite] $Message" }

function Test-IsAdmin {
    try {
        $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
        $principal = New-Object Security.Principal.WindowsPrincipal($identity)
        return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    } catch {
        return $false
    }
}

function Get-FullPath {
    param([string]$Path)
    return $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Path)
}

function Get-ManagedChildPath {
    param([string]$Root, [string]$Relative)
    if ([string]::IsNullOrWhiteSpace($Relative) -or [IO.Path]::IsPathRooted($Relative)) {
        return $null
    }
    $parts = @($Relative -split '[\\/]')
    if ($parts.Count -eq 0 -or $parts -contains '..' -or $parts -contains '.') {
        return $null
    }
    $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    $candidate = [IO.Path]::GetFullPath((Join-Path $rootFull $Relative))
    $prefix = $rootFull + '\'
    if (-not $candidate.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        return $null
    }
    return $candidate
}

function Test-ManagedParentChain {
    param([string]$Root, [string]$Path)
    $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    $current = Split-Path -Parent $Path
    while ($current -and -not $current.Equals($rootFull, [StringComparison]::OrdinalIgnoreCase)) {
        $item = Get-Item -LiteralPath $current -Force -ErrorAction SilentlyContinue
        if ($item -and (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
            return $false
        }
        $next = Split-Path -Parent $current
        if (-not $next -or $next -eq $current) { return $false }
        $current = $next
    }
    return $current -and $current.Equals($rootFull, [StringComparison]::OrdinalIgnoreCase)
}

function Read-InstallManifest {
    param([string]$Root)
    $marker = Join-Path $Root $Script:InstallManifestName
    $item = Get-Item -LiteralPath $marker -Force -ErrorAction SilentlyContinue
    if (-not $item) { return $null }
    if ($item.PSIsContainer -or (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
        throw "Refusing an unrecognized install manifest: $marker"
    }
    try {
        $manifest = [IO.File]::ReadAllText($marker) | ConvertFrom-Json
    } catch {
        throw "Refusing an invalid install manifest: $marker"
    }
    $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    if ($manifest.schema -ne $Script:InstallManifestSchema -or
        $manifest.name -ne 'tex-suite' -or
        -not $manifest.root -or
        -not ([IO.Path]::GetFullPath([string]$manifest.root).TrimEnd('\')).Equals(
            $rootFull, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing an install manifest that does not own this root: $marker"
    }
    foreach ($relative in @($manifest.files)) {
        if (-not ($relative -is [string]) -or -not (Get-ManagedChildPath -Root $Root -Relative $relative)) {
            throw "Install manifest contains an unsafe managed path: $relative"
        }
    }
    return $manifest
}

function Assert-OwnedDestinations {
    param([string]$Root, [string[]]$DesiredFiles, $ExistingManifest)
    $owned = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase)
    if ($ExistingManifest) {
        foreach ($relative in @($ExistingManifest.files)) { [void]$owned.Add([string]$relative) }
    }
    foreach ($relative in $DesiredFiles) {
        $path = Get-ManagedChildPath -Root $Root -Relative $relative
        if (-not $path) { throw "Refusing unsafe install destination: $relative" }
        if (-not (Test-ManagedParentChain -Root $Root -Path $path)) {
            throw "Refusing install destination through a reparse-point or unsafe parent: $path"
        }
        $item = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
        if ($item -and -not $owned.Contains($relative)) {
            throw "Refusing to overwrite unowned path: $path"
        }
        if ($item -and $item.PSIsContainer -and
            (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0)) {
            throw "Managed file destination is a directory: $path"
        }
    }
}

function Remove-ManagedInstallFiles {
    param([string]$Root, $Manifest, [switch]$KeepManifest)
    $directories = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase)
    foreach ($relative in @($Manifest.files)) {
        $path = Get-ManagedChildPath -Root $Root -Relative ([string]$relative)
        if (-not $path) { throw "Install manifest contains an unsafe managed path: $relative" }
        if (-not (Test-ManagedParentChain -Root $Root -Path $path)) {
            Write-Warning "Preserving managed path through a reparse-point or unsafe parent: $path"
            continue
        }
        $item = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
        if ($item) {
            if ($item.PSIsContainer -and
                (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0)) {
                Write-Warning "Preserving directory at managed file path: $path"
            } else {
                Remove-Item -LiteralPath $path -Force
                Write-Info "Removed $path"
            }
        }
        $parentRelative = Split-Path -Parent ([string]$relative)
        while ($parentRelative -and $parentRelative -ne '.') {
            [void]$directories.Add($parentRelative)
            $next = Split-Path -Parent $parentRelative
            if (-not $next -or $next -eq $parentRelative) { break }
            $parentRelative = $next
        }
    }
    foreach ($relative in @($directories | Sort-Object Length -Descending)) {
        $path = Get-ManagedChildPath -Root $Root -Relative $relative
        if (-not $path -or -not (Test-ManagedParentChain -Root $Root -Path $path)) { continue }
        $item = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
        if ($item -and $item.PSIsContainer -and
            (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) -and
            -not (Get-ChildItem -LiteralPath $path -Force -ErrorAction SilentlyContinue)) {
            Remove-Item -LiteralPath $path -Force
        }
    }
    if (-not $KeepManifest) {
        $marker = Join-Path $Root $Script:InstallManifestName
        Remove-Item -LiteralPath $marker -Force -ErrorAction SilentlyContinue
        foreach ($relative in @('bin', 'share\tex-suite', 'share')) {
            $path = Get-ManagedChildPath -Root $Root -Relative $relative
            $item = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
            if ($item -and $item.PSIsContainer -and
                (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) -and
                -not (Get-ChildItem -LiteralPath $path -Force -ErrorAction SilentlyContinue)) {
                Remove-Item -LiteralPath $path -Force
            }
        }
        $rootItem = Get-Item -LiteralPath $Root -Force -ErrorAction SilentlyContinue
        if ($rootItem -and $rootItem.PSIsContainer -and
            (($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) -and
            -not (Get-ChildItem -LiteralPath $Root -Force -ErrorAction SilentlyContinue)) {
            Remove-Item -LiteralPath $Root -Force
        }
    }
}

function Get-UserRoot {
    $base = $env:LOCALAPPDATA
    if (-not $base -and $env:USERPROFILE) {
        $base = Join-Path $env:USERPROFILE 'AppData\Local'
    }
    if (-not $base) {
        throw 'Cannot determine %LOCALAPPDATA%; pass -InstallDir explicitly.'
    }
    return (Join-Path $base 'tex-suite')
}

function Get-MachineRoot {
    if (-not $env:ProgramFiles) {
        throw 'Cannot determine %ProgramFiles%; pass -InstallDir explicitly.'
    }
    return (Join-Path $env:ProgramFiles 'tex-suite')
}

function Get-InstallRoot {
    param([switch]$ForUninstall)
    if ($InstallDir) { return (Get-FullPath $InstallDir) }
    if ($System)     { return (Get-MachineRoot) }
    return (Get-UserRoot)
}

# ---- persistent environment (registry-backed) handling -------------------

function Get-StoredPathValue {
    # Returns the PATH exactly as stored (unexpanded %VARS% preserved) for
    # the 'User' or 'Machine' scope, falling back to the .NET API.
    param([string]$Scope)
    $fallback = [Environment]::GetEnvironmentVariable('Path', [EnvironmentVariableTarget]::$Scope)
    if ($null -eq $fallback) { $fallback = '' }
    try {
        if ($Scope -eq 'Machine') {
            $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey(
                'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', $false)
        } else {
            $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $false)
        }
        if ($key) {
            $raw = $key.GetValue('Path', $null,
                [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $key.Close()
            if ($raw -is [string] -and $raw.Length -gt 0) { return $raw }
        }
    } catch { }
    return $fallback
}

function Set-StoredPathValue {
    param([string]$Scope, [string]$Value)
    # .NET stores REG_EXPAND_SZ when the value contains '%', preserving
    # unexpanded variables, and broadcasts WM_SETTINGCHANGE automatically.
    [Environment]::SetEnvironmentVariable('Path', $Value, [EnvironmentVariableTarget]::$Scope)
}

function Add-ToPathIdempotent {
    param([string]$Scope, [string]$Dir)
    $norm = $Dir.TrimEnd('\')
    $cur = Get-StoredPathValue $Scope
    $parts = @($cur -split ';' | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' })
    $exists = $false
    foreach ($p in $parts) {
        if ($p.TrimEnd('\') -ieq $norm) { $exists = $true; break }
    }
    if (-not $exists) {
        # Also compare the expanded form (covers %USERPROFILE%-style entries).
        $expanded = [Environment]::GetEnvironmentVariable('Path', [EnvironmentVariableTarget]::$Scope)
        if ($expanded) {
            foreach ($p in ($expanded -split ';')) {
                if ($p.Trim().TrimEnd('\') -ieq $norm) { $exists = $true; break }
            }
        }
    }
    if ($exists) {
        Write-Info "$Dir is already on the $Scope PATH; PATH left unchanged."
        return
    }
    $new = ($parts + $Dir) -join ';'
    Set-StoredPathValue $Scope $new
    Write-Info "Added $Dir to the $Scope PATH."
}

function Remove-FromPathIdempotent {
    param([string]$Scope, [string]$Dir)
    $norm = $Dir.TrimEnd('\')
    $cur = Get-StoredPathValue $Scope
    if (-not $cur) { return }
    $parts = @($cur -split ';' | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' })
    # Drop entries equal to $Dir, either literally or after %VARS% expansion.
    $kept = @($parts | Where-Object {
        $v = $_.TrimEnd('\')
        $expanded = ([Environment]::ExpandEnvironmentVariables($v)).TrimEnd('\')
        -not (($v -ieq $norm) -or ($expanded -ieq $norm))
    })
    if ($kept.Count -eq $parts.Count) { return }  # nothing to remove
    if ($kept.Count -eq 0 -and $Scope -eq 'Machine') {
        Write-Info 'Refusing to empty the Machine PATH; leaving it unchanged.'
        return
    }
    Set-StoredPathValue $Scope ($kept -join ';')
    Write-Info "Removed $Dir from the $Scope PATH."
}

function Set-EnvVarPersistent {
    param([string]$Scope, [string]$Name, [string]$Value)
    [Environment]::SetEnvironmentVariable($Name, $Value, [EnvironmentVariableTarget]::$Scope)
    Write-Info "Set $Scope $Name = $Value"
}

function Clear-EnvVarIfOurs {
    param([string]$Scope, [string]$Name, [string]$RootPrefix)
    $cur = [Environment]::GetEnvironmentVariable($Name, [EnvironmentVariableTarget]::$Scope)
    if (-not $cur) { return }
    $prefix = $RootPrefix.TrimEnd('\')
    if ($cur.TrimEnd('\') -ieq $prefix -or $cur.TrimEnd('\') -like ($prefix + '\*')) {
        [Environment]::SetEnvironmentVariable($Name, $null, [EnvironmentVariableTarget]::$Scope)
        Write-Info "Removed $Scope $Name."
    } else {
        Write-Info "Left $Scope $Name unchanged (points elsewhere: $cur)."
    }
}

# ---- source discovery -----------------------------------------------------

function Find-Source {
    param([string]$Explicit)
    $candidates = @()
    if ($Explicit) { $candidates += $Explicit }
    if ($PSScriptRoot) {
        $candidates += $PSScriptRoot
        $candidates += (Split-Path -Parent $PSScriptRoot)
    }
    $candidates += (Get-Location).Path

    # Prefer a staged bundle: <root>\bin\texmk.exe + <root>\share\tex-suite.
    foreach ($c in $candidates) {
        if (-not $c) { continue }
        $bin = Join-Path $c 'bin'
        if ((Test-Path -LiteralPath (Join-Path $bin 'ratex.exe')) -or (Test-Path -LiteralPath (Join-Path $bin 'texmk.exe'))) {
            $data = Join-Path $c (Join-Path 'share' 'tex-suite')
            return [pscustomobject]@{
                Mode   = 'bundle'
                Root   = $c
                BinDir = $bin
                Fmt    = (Join-Path $data 'pdflatex.fmt')
                Texmf  = (Join-Path $data 'texmf')
            }
        }
    }

    # Fall back to a Cargo checkout: <root>\target\release\*.exe.
    foreach ($c in $candidates) {
        if (-not $c) { continue }
        $rel = Join-Path $c 'target\release'
        if ((Test-Path -LiteralPath (Join-Path $rel 'ratex.exe')) -or (Test-Path -LiteralPath (Join-Path $rel 'texmk.exe'))) {
            $texmf = $null
            foreach ($t in @((Join-Path $c (Join-Path 'share\tex-suite' 'texmf')), (Join-Path $c 'texmf'))) {
                if (Test-Path -LiteralPath $t) { $texmf = $t; break }
            }
            return [pscustomobject]@{
                Mode   = 'repo'
                Root   = $c
                BinDir = $rel
                Fmt    = $null
                Texmf  = $texmf
            }
        }
    }
    return $null
}

# ---- copy helpers ----------------------------------------------------------

function Copy-ExeTo {
    param([string]$Src, [string]$Dst)
    try {
        Copy-Item -LiteralPath $Src -Destination $Dst -Force
    } catch {
        $msg = $_.Exception.Message
        if ($msg -match 'being used by another process|Access is denied') {
            throw ("Cannot write '{0}' ({1}). Close any running tex-suite " +
                   'commands and try again.') -f (Split-Path -Leaf $Dst), $msg
        }
        throw
    }
}

function Write-CmdWrapper {
    param([string]$BinDir, [string]$WrapperBaseName, [string]$TargetExe)
    $path = Join-Path $BinDir ($WrapperBaseName + '.cmd')
    $content = ("@echo off`r`n" +
                "rem tex-suite wrapper: forwards to $TargetExe`r`n" +
                "`"%~dp0$TargetExe`" %*`r`n")
    [IO.File]::WriteAllText($path, $content, [Text.Encoding]::ASCII)
    Write-Info "Created helper wrapper $path (forwards to $TargetExe)."
}

function Copy-Tree {
    param([string]$Src, [string]$Dst)
    New-Item -ItemType Directory -Force -Path $Dst | Out-Null
    try {
        & robocopy $Src $Dst /E /NFL /NDL /NJH /NJS /NP /R:2 /W:1 | Out-Null
        $rc = $LASTEXITCODE
        $global:LASTEXITCODE = 0
        if ($rc -ge 8) {
            throw "robocopy failed (exit code $rc) copying $Src to $Dst"
        }
    } catch [System.Management.Automation.CommandNotFoundException] {
        Copy-Item -LiteralPath $Src -Destination $Dst -Recurse -Force
    }
}

# ---- verification ----------------------------------------------------------

function Invoke-Verify {
    param([string]$Exe)
    # The engines use TeX-style single-dash flags: `pdflatex -version` prints
    # the version banner; `--version` is accepted as a fallback.
    $tmp = [IO.Path]::GetTempPath()
    $outFile = Join-Path $tmp ('tex-suite-verify-' + [guid]::NewGuid().ToString('N') + '.out')
    $errFile = $outFile + '.err'
    foreach ($flag in @('-version', '--version')) {
        try {
            $proc = Start-Process -FilePath $Exe -ArgumentList $flag -NoNewWindow `
                -Wait -PassThru -WorkingDirectory $tmp `
                -RedirectStandardOutput $outFile -RedirectStandardError $errFile
            $text = ''
            if (Test-Path -LiteralPath $outFile) {
                $text = [IO.File]::ReadAllText($outFile).Trim()
            }
            if ($proc.ExitCode -eq 0 -and $text -ne '') {
                Write-Info "Verification OK: pdflatex $flag ->"
                foreach ($line in ($text -split "`r?`n" | Select-Object -First 3)) {
                    Write-Host "    $line"
                }
                Remove-Item -LiteralPath $outFile, $errFile -Force -ErrorAction SilentlyContinue
                return $true
            }
        } catch { }
    }
    Remove-Item -LiteralPath $outFile, $errFile -Force -ErrorAction SilentlyContinue
    return $false
}

# ---- help ------------------------------------------------------------------

function Show-Usage {
    $script = if ($PSScriptRoot) { Join-Path $PSScriptRoot 'install-windows.ps1' } else { 'install-windows.ps1' }
    $text = @"

tex-suite installer (Windows)  --  pdflatex / xelatex / lualatex / bibtex / texmk

USAGE
  powershell -NoProfile -ExecutionPolicy Bypass -File "$script" [options]
  (double-click install.bat next to this script for the same thing)

OPTIONS
  (no options)    Install for the current user into
                  %LOCALAPPDATA%\tex-suite
  -System         Install machine-wide into %ProgramFiles%\tex-suite and edit
                  the Machine environment (requires an elevated prompt).
  -Uninstall      Remove installed files, the PATH entry, and TEXMFLOCAL /
                  TEX_SUITE_DATA. Combine with -System for a machine install.
  -SourceDir <p>  Staged bundle to install from (expects <p>\bin\*.exe and
                  <p>\share\tex-suite\texmf\). Default: the
                  directory containing this script, else a Cargo checkout
                  (target\release) in the current/parent directories.
  -InstallDir <p> Install root override (files go to <p>\bin and
                  <p>\share\tex-suite).
  -AliasLatexmk   Install 'latexmk.exe' alias pointing to texmk (default).
  -NoAliasLatexmk Do not install 'latexmk.exe' alias.
  -Help           Show this message.

WHAT IT DOES
  1. Copies canonical engines and small alias launchers to <root>\bin.
  2. Copies the texmf tree to <root>\share\tex-suite. The compressed LaTeX
     format is embedded in pdflatex.exe; an external format is optional.
     TEXMFLOCAL points at <root>\share\tex-suite\texmf.
  3. Adds <root>\bin to the persistent User (or Machine) PATH idempotently.
  4. Verifies the install by running: pdflatex.exe -version

After installing, open a NEW terminal so the updated PATH is picked up.

"@
    Write-Host $text
}

# ---- install ---------------------------------------------------------------

function Install-Suite {
    $admin = Test-IsAdmin
    $scope = 'User'
    if ($System) {
        if (-not $admin) {
            throw '-System requires an elevated (Administrator) PowerShell or Command Prompt.'
        }
        $scope = 'Machine'
    }

    $root = Get-InstallRoot
    $bin = Join-Path $root 'bin'
    $data = Join-Path $root (Join-Path 'share' 'tex-suite')

    $src = Find-Source -Explicit $SourceDir
    if (-not $src) {
        throw ("Could not find tex-suite build output. Expected a staged bundle " +
               "(bin\pdflatex.exe + share\tex-suite\) next to this script, or a " +
               'Cargo checkout with target\release\pdflatex.exe. Run ' +
               '"cargo build --release --workspace" first, or pass -SourceDir.')
    }
    Write-Info ("Installing from " + $src.Mode + " source: " + $src.BinDir)
    Write-Info "Install root: $root"

    New-Item -ItemType Directory -Force -Path $root | Out-Null
    $rootItem = Get-Item -LiteralPath $root -Force
    if (-not $rootItem.PSIsContainer -or
        (($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
        throw "Refusing an install root that is not a plain directory: $root"
    }
    foreach ($directory in @($bin, $data)) {
        $probe = Join-Path $directory '.tex-suite-parent-check'
        if (-not (Test-ManagedParentChain -Root $root -Path $probe)) {
            throw "Refusing an install path through a reparse-point or unsafe parent: $directory"
        }
    }
    New-Item -ItemType Directory -Force -Path $bin | Out-Null
    New-Item -ItemType Directory -Force -Path $data | Out-Null

    $existingManifest = Read-InstallManifest -Root $root
    $desiredFiles = New-Object 'System.Collections.Generic.List[string]'
    foreach ($name in ($Script:CoreExes + $Script:ExtraExes)) {
        if (Test-Path -LiteralPath (Join-Path $src.BinDir $name)) {
            [void]$desiredFiles.Add((Join-Path 'bin' $name))
        }
    }
    foreach ($shim in $Script:Shims) {
        $stagedAlias = Join-Path $src.BinDir $shim.Name
        $stagedParent = Join-Path $src.BinDir $shim.Parent
        if ((Test-Path -LiteralPath $stagedAlias) -or (Test-Path -LiteralPath $stagedParent)) {
            [void]$desiredFiles.Add((Join-Path 'bin' $shim.Name))
        } else {
            $wrapper = [IO.Path]::GetFileNameWithoutExtension($shim.Name) + '.cmd'
            [void]$desiredFiles.Add((Join-Path 'bin' $wrapper))
        }
    }
    if ($src.Fmt -and (Test-Path -LiteralPath $src.Fmt)) {
        [void]$desiredFiles.Add((Join-Path 'share\tex-suite' 'pdflatex.fmt'))
        [void]$desiredFiles.Add((Join-Path 'bin' 'pdflatex.fmt'))
    }
    $sourceTexmfFiles = @()
    $sourceTexmfDirs = @()
    if ($src.Texmf -and (Test-Path -LiteralPath $src.Texmf)) {
        $sourceRoot = [IO.Path]::GetFullPath($src.Texmf).TrimEnd('\')
        $sourceTexmfFiles = @(Get-ChildItem -LiteralPath $src.Texmf -File -Recurse -Force)
        $sourceTexmfDirs = @(Get-ChildItem -LiteralPath $src.Texmf -Directory -Recurse -Force)
        foreach ($fileItem in $sourceTexmfFiles) {
            $relative = $fileItem.FullName.Substring($sourceRoot.Length).TrimStart('\')
            [void]$desiredFiles.Add((Join-Path 'share\tex-suite\texmf' $relative))
        }
    }
    Assert-OwnedDestinations -Root $root -DesiredFiles @($desiredFiles) -ExistingManifest $existingManifest
    $owned = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase)
    if ($existingManifest) {
        foreach ($relative in @($existingManifest.files)) { [void]$owned.Add([string]$relative) }
    }
    if (-not ($src.Fmt -and (Test-Path -LiteralPath $src.Fmt))) {
        foreach ($relative in @('bin\pdflatex.fmt', 'share\tex-suite\pdflatex.fmt')) {
            $path = Get-ManagedChildPath -Root $root -Relative $relative
            if ((Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue) -and
                -not $owned.Contains($relative)) {
                throw "Unowned format override blocks the embedded format: $path"
            }
        }
    }
    foreach ($sourceDirectory in $sourceTexmfDirs) {
        $relative = $sourceDirectory.FullName.Substring($sourceRoot.Length).TrimStart('\')
        $destination = Get-ManagedChildPath -Root $root -Relative (Join-Path 'share\tex-suite\texmf' $relative)
        if (-not (Test-ManagedParentChain -Root $root -Path (Join-Path $destination '.tex-suite-parent-check'))) {
            throw "Refusing texmf destination through a reparse-point or unsafe parent: $destination"
        }
        $item = Get-Item -LiteralPath $destination -Force -ErrorAction SilentlyContinue
        if ($item -and (-not $item.PSIsContainer -or
            (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0))) {
            throw "Refusing non-directory or reparse-point texmf destination: $destination"
        }
    }
    if ($existingManifest) {
        Remove-ManagedInstallFiles -Root $root -Manifest $existingManifest -KeepManifest
    }
    $managedFiles = New-Object 'System.Collections.Generic.List[string]'

    # 1. Engine binaries
    foreach ($name in ($Script:CoreExes + $Script:ExtraExes)) {
        $s = Join-Path $src.BinDir $name
        if (-not (Test-Path -LiteralPath $s)) {
            if ($Script:CoreExes -contains $name) {
                throw "Required binary not found in source: $s"
            }
            Write-Warning "Optional binary not found in source, skipping: $name"
            continue
        }
        Copy-ExeTo -Src $s -Dst (Join-Path $bin $name)
        [void]$managedFiles.Add((Join-Path 'bin' $name))
        Write-Info "Installed $name"
    }

    # 2. Aliases. A current bundle/source build contains the correctly named
    # launcher. Older payloads contain full, personality-aware executables;
    # copying each alias by its own name preserves those payloads too.
    foreach ($shim in $Script:Shims) {
        $parent = Join-Path $bin $shim.Parent
        $stagedAlias = Join-Path $src.BinDir $shim.Name
        if (Test-Path -LiteralPath $stagedAlias) {
            Copy-ExeTo -Src $stagedAlias -Dst (Join-Path $bin $shim.Name)
            [void]$managedFiles.Add((Join-Path 'bin' $shim.Name))
            Write-Info ("Installed " + $shim.Name + ' (alias executable)')
        } elseif (Test-Path -LiteralPath $parent) {
            Copy-ExeTo -Src $parent -Dst (Join-Path $bin $shim.Name)
            [void]$managedFiles.Add((Join-Path 'bin' $shim.Name))
            Write-Warning ("Installed " + $shim.Name + " as a full copy of " + $shim.Parent)
        } else {
            # Parent missing (e.g. not built): fall back to a .cmd wrapper
            # pointing at the parent if it ever shows up next to the bin dir.
            Write-CmdWrapper -BinDir $bin `
                -WrapperBaseName ([IO.Path]::GetFileNameWithoutExtension($shim.Name)) `
                -TargetExe $shim.Parent
            [void]$managedFiles.Add((Join-Path 'bin' ([IO.Path]::GetFileNameWithoutExtension($shim.Name) + '.cmd')))
        }
    }

    # 3. Runtime data: optional external format + texmf TDS tree
    if ($src.Fmt -and (Test-Path -LiteralPath $src.Fmt)) {
        Copy-ExeTo -Src $src.Fmt -Dst (Join-Path $data 'pdflatex.fmt')
        Copy-ExeTo -Src $src.Fmt -Dst (Join-Path $bin 'pdflatex.fmt')
        [void]$managedFiles.Add((Join-Path 'share\tex-suite' 'pdflatex.fmt'))
        [void]$managedFiles.Add((Join-Path 'bin' 'pdflatex.fmt'))
        Write-Info 'Installed pdflatex.fmt'
    } else {
        # Remove the two exact locations written by pre-embedded-format
        # installers. A directory or any other unexpected object is retained.
        foreach ($legacyFmt in @((Join-Path $bin 'pdflatex.fmt'),
                                 (Join-Path $data 'pdflatex.fmt'))) {
            $legacyItem = Get-Item -LiteralPath $legacyFmt -Force -ErrorAction SilentlyContinue
            if ($legacyItem -and -not $legacyItem.PSIsContainer) {
                Remove-Item -LiteralPath $legacyFmt -Force
                Write-Info "Removed legacy $legacyFmt"
            } elseif ($legacyItem) {
                Write-Warning "Legacy format path is not a file; leaving it unchanged: $legacyFmt"
            }
        }
        Write-Info 'Using the compressed LaTeX format embedded in pdflatex.exe'
    }
    if ($src.Texmf -and (Test-Path -LiteralPath $src.Texmf)) {
        $texmfDst = Join-Path $data 'texmf'
        $texmfItem = Get-Item -LiteralPath $texmfDst -Force -ErrorAction SilentlyContinue
        if ($texmfItem -and (($texmfItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
            throw "Refusing to install through a reparse-point texmf directory: $texmfDst"
        }
        if ($texmfItem) {
            $nestedReparse = Get-ChildItem -LiteralPath $texmfDst -Directory -Recurse -Force -ErrorAction SilentlyContinue |
                Where-Object { ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 } |
                Select-Object -First 1
            if ($nestedReparse) {
                throw "Refusing to update a texmf tree containing a reparse point: $($nestedReparse.FullName)"
            }
        }
        Copy-Tree -Src $src.Texmf -Dst $texmfDst
        $sourceRoot = [IO.Path]::GetFullPath($src.Texmf).TrimEnd('\')
        foreach ($fileItem in $sourceTexmfFiles) {
            $relative = $fileItem.FullName.Substring($sourceRoot.Length).TrimStart('\')
            [void]$managedFiles.Add((Join-Path 'share\tex-suite\texmf' $relative))
        }
        Write-Info "Installed texmf tree from $($src.Texmf)"
    } else {
        Write-Warning 'No texmf directory found in the source; skipping TDS data.'
    }

    # 4. Exact ownership manifest. Uninstall consumes only these relative file
    # paths and then removes directories non-recursively if they are empty.
    $meta = [ordered]@{
        schema = $Script:InstallManifestSchema
        name = 'tex-suite'
        scope = $scope
        root = $root
        installed = (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
        files = @($managedFiles | Sort-Object -Unique)
    } | ConvertTo-Json -Depth 4
    [IO.File]::WriteAllText((Join-Path $root $Script:InstallManifestName), $meta)

    # 5. Persistent environment.
    #    TEXMFLOCAL is the variable the engines (tex-kpse) actually read:
    #    a semicolon-separated list of TDS roots. TEX_SUITE_DATA is set as a
    #    convenience pointer to the data directory for other tooling.
    $texmfDst = Join-Path $data 'texmf'
    if (-not $NoPath) {
        Set-EnvVarPersistent -Scope $scope -Name 'TEXMFLOCAL' -Value $texmfDst
        Set-EnvVarPersistent -Scope $scope -Name 'TEX_SUITE_DATA' -Value $data
        Add-ToPathIdempotent -Scope $scope -Dir $bin

        # Make the current session usable immediately (children only).
        $env:TEXMFLOCAL = $texmfDst
        $env:TEX_SUITE_DATA = $data
        if (($env:Path -split ';' | ForEach-Object { $_.Trim().TrimEnd('\') }) -inotcontains $bin.TrimEnd('\')) {
            $env:Path = $bin + ';' + $env:Path
        }
    }
    # 6. Verify.
    $ratexExe = Join-Path $bin 'ratex.exe'
    if ((-not $SkipVerify) -and (Test-Path -LiteralPath $ratexExe)) {
        if (Invoke-Verify -Exe $ratexExe) {
            Write-Info 'Installation verified.'
        } else {
            Write-Warning ('ratex.exe -version did not return a version ' +
                           'banner. Files were installed; run the executable ' +
                           'from a terminal to inspect its diagnostic.')
        }
    }
    Write-Info 'Done. Open a NEW terminal, then try: ratex -version'
}

# ---- uninstall ---------------------------------------------------------------

function Remove-InstallTree {
    param([string]$Root)
    if (-not (Test-Path -LiteralPath $Root)) { return }
    $rootItem = Get-Item -LiteralPath $Root -Force
    if (-not $rootItem.PSIsContainer -or
        (($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
        throw "Refusing to uninstall through a non-directory or reparse-point root: $Root"
    }
    $manifest = Read-InstallManifest -Root $Root
    if (-not $manifest) {
        Write-Warning "No valid ownership manifest at $Root; preserving install and data files."
        return
    }
    Remove-ManagedInstallFiles -Root $Root -Manifest $manifest
}

function Uninstall-Suite {
    $admin = Test-IsAdmin
    $roots = @()
    $scopes = @('User')

    if ($InstallDir) {
        $roots += (Get-FullPath $InstallDir)
        if ($System) { $scopes = @('Machine') }
    } elseif ($System) {
        if (-not $admin) {
            throw '-System requires an elevated (Administrator) PowerShell or Command Prompt.'
        }
        $roots += Get-MachineRoot
        $scopes = @('Machine')
    } else {
        $roots += Get-UserRoot
        $roots += Get-MachineRoot   # also clean a machine install, if admin
    }
    if (-not $admin) { $scopes = @('User') }

    foreach ($root in ($roots | Select-Object -Unique)) {
        $bin = Join-Path $root 'bin'
        foreach ($scope in $scopes) {
            Remove-FromPathIdempotent -Scope $scope -Dir $bin
            Clear-EnvVarIfOurs -Scope $scope -Name 'TEXMFLOCAL' -RootPrefix $root
            Clear-EnvVarIfOurs -Scope $scope -Name 'TEX_SUITE_DATA' -RootPrefix $root
        }
        Remove-InstallTree -Root $root
    }
    if (-not $admin) {
        Write-Info 'Note: not running as Administrator; Machine-scope PATH/env entries were not checked. Re-run with -System from an elevated prompt if you installed machine-wide.'
    }
    Write-Info 'Uninstall complete. Reopen any terminals to see the cleaned environment.'
}

# ---- main --------------------------------------------------------------------

$exitCode = 0
try {
    if ($Help) {
        Show-Usage
    } elseif ($Uninstall) {
        Uninstall-Suite
    } else {
        Install-Suite
    }
} catch {
    Write-Host "[tex-suite] ERROR: $($_.Exception.Message)" -ForegroundColor Red
    if ($_.InvocationInfo -and $_.InvocationInfo.ScriptLineNumber) {
        Write-Host "[tex-suite]        at line $($_.InvocationInfo.ScriptLineNumber) of $($_.InvocationInfo.ScriptName)" -ForegroundColor Red
    }
    $exitCode = 1
}
exit $exitCode
