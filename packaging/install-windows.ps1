<#
.SYNOPSIS
    Installer for the tex-suite Rust TeX engine collection on Windows.

.DESCRIPTION
    Self-contained installer for pdflatex, xelatex, lualatex, bibtex and
    texmk/latexmk. Copies the release binaries into <root>\bin, the runtime
    data (pdflatex.fmt and the texmf TDS tree) into <root>\share\tex-suite,
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
    SourceDir\share\tex-suite\{pdflatex.fmt,texmf}. By default the script
    looks next to itself (bundle layout), then for a Cargo checkout
    (target\release + pdflatex.fmt at the repo root).

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
    [string]$SourceDir = '',
    [string]$InstallDir = ''
)

$ErrorActionPreference = 'Stop'

# Allow "run by pasting into a console" where $PSScriptRoot is not defined.
if (-not $PSScriptRoot) {
    $PSScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
}

# --------------------------------------------------------------- constants

$Script:CoreExes = @('pdflatex.exe', 'xelatex.exe', 'lualatex.exe')
$Script:ExtraExes = @('tex-bibtex.exe', 'texmk.exe', 'tex-index.exe')
# Shim name -> parent engine (shim is a byte-identical copy; the engines
# inspect their own argv[0] to pick personality, so a copy is required).
$Script:Shims = @(
    @{ Name = 'bibtex.exe';  Parent = 'tex-bibtex.exe' },
    @{ Name = 'latexmk.exe'; Parent = 'texmk.exe' }
)
$Script:AllInstalledExes = $Script:CoreExes + $Script:ExtraExes + ($Script:Shims | ForEach-Object { $_.Name })
$Script:EnvVars = @('TEXMFLOCAL', 'TEX_SUITE_DATA')

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

    # Prefer a staged bundle: <root>\bin\pdflatex.exe + <root>\share\tex-suite.
    foreach ($c in $candidates) {
        if (-not $c) { continue }
        $bin = Join-Path $c 'bin'
        if (Test-Path -LiteralPath (Join-Path $bin 'pdflatex.exe')) {
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
        if (Test-Path -LiteralPath (Join-Path $rel 'pdflatex.exe')) {
            $fmt = Join-Path $c 'pdflatex.fmt'
            if (-not (Test-Path -LiteralPath $fmt)) {
                $alt = Join-Path $rel 'pdflatex.fmt'
                if (Test-Path -LiteralPath $alt) { $fmt = $alt }
            }
            $texmf = $null
            foreach ($t in @((Join-Path $c (Join-Path 'share\tex-suite' 'texmf')), (Join-Path $c 'texmf'))) {
                if (Test-Path -LiteralPath $t) { $texmf = $t; break }
            }
            return [pscustomobject]@{
                Mode   = 'repo'
                Root   = $c
                BinDir = $rel
                Fmt    = $fmt
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
                  <p>\share\tex-suite\pdflatex.fmt + texmf\). Default: the
                  directory containing this script, else a Cargo checkout
                  (target\release) in the current/parent directories.
  -InstallDir <p> Install root override (files go to <p>\bin and
                  <p>\share\tex-suite).
  -Help           Show this message.

WHAT IT DOES
  1. Copies engine binaries to <root>\bin (tex-bibtex.exe is also copied as
     bibtex.exe; texmk.exe also as latexmk.exe).
  2. Copies pdflatex.fmt and the texmf tree to <root>\share\tex-suite.
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

    New-Item -ItemType Directory -Force -Path $bin | Out-Null
    New-Item -ItemType Directory -Force -Path $data | Out-Null

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
        Write-Info "Installed $name"
    }

    # 2. Shims: byte-identical copies so argv[0] personality detection works.
    foreach ($shim in $Script:Shims) {
        $parent = Join-Path $bin $shim.Parent
        if (Test-Path -LiteralPath $parent) {
            Copy-ExeTo -Src $parent -Dst (Join-Path $bin $shim.Name)
            Write-Info ("Installed " + $shim.Name + " (copy of " + $shim.Parent + ')')
        } else {
            # Parent missing (e.g. not built): fall back to a .cmd wrapper
            # pointing at the parent if it ever shows up next to the bin dir.
            Write-CmdWrapper -BinDir $bin `
                -WrapperBaseName ([IO.Path]::GetFileNameWithoutExtension($shim.Name)) `
                -TargetExe $shim.Parent
        }
    }

    # 3. Runtime data: format file + texmf TDS tree
    if ($src.Fmt -and (Test-Path -LiteralPath $src.Fmt)) {
        Copy-ExeTo -Src $src.Fmt -Dst (Join-Path $data 'pdflatex.fmt')
        Write-Info 'Installed pdflatex.fmt'
    } else {
        Write-Warning ('pdflatex.fmt not found (' + $src.Fmt +
                       '). Generate it with: pdflatex -ini latex.ltx')
    }
    if ($src.Texmf -and (Test-Path -LiteralPath $src.Texmf)) {
        Copy-Tree -Src $src.Texmf -Dst (Join-Path $data 'texmf')
        Write-Info "Installed texmf tree from $($src.Texmf)"
    } else {
        Write-Warning 'No texmf directory found in the source; skipping TDS data.'
    }

    # 4. Marker so uninstall can recognise (and fully remove) our tree.
    $meta = '{"name":"tex-suite","scope":"' + $scope +
            '","root":"' + $root.Replace('\', '\\') +
            '","installed":"' + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss') + '"}'
    [IO.File]::WriteAllText((Join-Path $root '.tex-suite-install.json'), $meta)

    # 5. Persistent environment.
    #    TEXMFLOCAL is the variable the engines (tex-kpse) actually read:
    #    a semicolon-separated list of TDS roots. TEX_SUITE_DATA is set as a
    #    convenience pointer to the data directory for other tooling.
    $texmfDst = Join-Path $data 'texmf'
    Set-EnvVarPersistent -Scope $scope -Name 'TEXMFLOCAL' -Value $texmfDst
    Set-EnvVarPersistent -Scope $scope -Name 'TEX_SUITE_DATA' -Value $data
    Add-ToPathIdempotent -Scope $scope -Dir $bin

    # Make the current session usable immediately (children only).
    $env:TEXMFLOCAL = $texmfDst
    $env:TEX_SUITE_DATA = $data
    if (($env:Path -split ';' | ForEach-Object { $_.Trim().TrimEnd('\') }) -inotcontains $bin.TrimEnd('\')) {
        $env:Path = $bin + ';' + $env:Path
    }

    # 6. Verify.
    $pdflatexExe = Join-Path $bin 'pdflatex.exe'
    if (Test-Path -LiteralPath $pdflatexExe) {
        if (Invoke-Verify -Exe $pdflatexExe) {
            Write-Info 'Installation verified.'
        } else {
            Write-Warning ('pdflatex.exe -version did not return a version ' +
                           'banner. Files were installed; check that a format ' +
                           'file is reachable via TEXMFLOCAL.')
        }
    }

    Write-Info 'Done. Open a NEW terminal, then try: pdflatex -version'
}

# ---- uninstall ---------------------------------------------------------------

function Remove-InstallTree {
    param([string]$Root)
    if (-not (Test-Path -LiteralPath $Root)) { return }
    $marker = Join-Path $Root '.tex-suite-install.json'
    if (Test-Path -LiteralPath $marker) {
        Remove-Item -LiteralPath $Root -Recurse -Force
        Write-Info "Removed $Root"
        return
    }
    # No marker: only remove entries we know we placed there.
    $bin = Join-Path $Root 'bin'
    if (Test-Path -LiteralPath $bin) {
        $shimNames = @($Script:Shims | ForEach-Object { $_.Name })
        $shimWrappers = @($shimNames | ForEach-Object { [IO.Path]::GetFileNameWithoutExtension($_) + '.cmd' })
        foreach ($f in (Get-ChildItem -LiteralPath $bin -File)) {
            if (($Script:AllInstalledExes -contains $f.Name) -or
                ($f.Extension -eq '.cmd' -and $shimWrappers -contains $f.Name)) {
                Remove-Item -LiteralPath $f.FullName -Force
                Write-Info "Removed $($f.FullName)"
            }
        }
        if (-not (Get-ChildItem -LiteralPath $bin -Recurse -Force -ErrorAction SilentlyContinue)) {
            Remove-Item -LiteralPath $bin -Recurse -Force
        }
    }
    $data = Join-Path $Root (Join-Path 'share' 'tex-suite')
    if (Test-Path -LiteralPath $data) {
        $known = @('pdflatex.fmt', 'texmf')
        $ours = $true
        foreach ($item in (Get-ChildItem -LiteralPath $data -Force)) {
            if ($known -notcontains $item.Name) { $ours = $false; break }
        }
        if ($ours) {
            Remove-Item -LiteralPath $data -Recurse -Force
            Write-Info "Removed $data"
        } else {
            Write-Warning "Not removing ${data}: contains files we did not install."
        }
    }
    # Remove the install root itself if it is now empty.
    if (Test-Path -LiteralPath $Root) {
        if (-not (Get-ChildItem -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue)) {
            Remove-Item -LiteralPath $Root -Recurse -Force
            Write-Info "Removed $Root"
        }
    }
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
