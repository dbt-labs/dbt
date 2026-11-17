# Uninstall script for dbt
[CmdletBinding()]
param(
    # Remove <Prefix>\bin\dbt.exe and its drivers in <Prefix>\lib\dbt. Defaults to ~\.local.
    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [ValidateScript({
        if(-not (Test-Path -Path $_ -IsValid)) {
            throw "Path '$_' contains invalid characters"
        }
        return $true
    })]
    [string]$Prefix,

    # Deprecated: the directory holding dbt.exe. Use -Prefix.
    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [ValidateScript({
        if(-not (Test-Path -Path $_ -IsValid)) {
            throw "Path '$_' contains invalid characters"
        }
        return $true
    })]
    [string]$installLocation,

    # Overrides installLocation\dbt.exe with the exact binary to remove --
    # older callers that only pass installLocation assume the binary is
    # always literally named dbt.exe, which isn't true for a renamed copy
    # (e.g. the VS Code extension's dbtf.exe, to avoid colliding with a
    # separate OSS dbt on PATH). Optional and additive so this script keeps
    # working unchanged for any already-installed dbt that only knows about
    # installLocation.
    [Parameter()]
    [ValidateScript({
        if ($_ -and -not (Test-Path -Path $_ -IsValid)) {
            throw "Path '$_' contains invalid characters"
        }
        return $true
    })]
    [string]$BinaryPath = ''
)

# Color support
$Host.UI.RawUI.WindowTitle = 'dbt Uninstaller'

# Get script path for cleanup
$scriptPath = $MyInvocation.MyCommand.Path

#region Logging Functions
function Write-Log {
    param($Message)
    Write-Host ('uninstall.ps1: ' + $Message.Replace("\\", "\"))
    [Console]::Out.Flush()
}

function Write-GrayLog {
    param($Message)
    $prevColor = $Host.UI.RawUI.ForegroundColor
    $Host.UI.RawUI.ForegroundColor = "DarkGray"
    [Console]::Error.WriteLine('uninstall.ps1: ' + $Message.Replace("\\", "\"))
    $Host.UI.RawUI.ForegroundColor = $prevColor
}

function Write-Debug {
    param($Message)
    Write-GrayLog ('DEBUG ' + $Message)
}
#endregion

#region Error Handling Functions
function Write-ErrorAndExit {
    param(
        [string]$ErrorMessage,
        [string]$AdditionalInfo = ''
    )

    # Clear any active progress bars
    Write-Progress -Activity '*' -Status 'Failed' -Completed

    # Log additional info if provided
    if ($AdditionalInfo) {
        Write-GrayLog $AdditionalInfo
    }

    # Log the error in red (PowerShell standard for errors)
    Write-Host 'uninstall.ps1: ERROR ' -ForegroundColor Red -NoNewline
    Write-Host ($ErrorMessage.Replace("\\", "\")) -ForegroundColor Red

    # Clean up the script itself
    if ($scriptPath -and (Test-Path $scriptPath)) {
        Remove-Item -Path $scriptPath -Force -ErrorAction SilentlyContinue
    }

    # Stop script execution
    throw
}
#endregion

#region System Check Functions
function Test-Need {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory=$true)]
        [string]$Command
    )

    if (-not (Get-Command $Command -ErrorAction SilentlyContinue)) {
        Write-ErrorAndExit "Required command '$Command' not found"
    }
}

function Test-RequiredCommands {
    $commands = @(
        'Remove-Item',
        'Test-Path',
        'Join-Path'
    )

    foreach ($command in $commands) {
        Test-Need -Command $command
    }
}

function Test-FileLock {
    param(
        [string]$Path
    )
    try {
        $file = [System.IO.File]::Open($Path, 'Open', 'Read', 'None')
        $file.Close()
        $file.Dispose()
        return $false
    } catch {
        return $true
    }
}
#endregion

#region Installation Helper Functions
function Set-InstallLocation {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory=$true)]
        [ValidateNotNullOrEmpty()]
        [ValidateScript({
            if(-not (Test-Path -Path $_ -IsValid)) {
                throw "Path '$_' contains invalid characters"
            }
            return $true
        })]
        [string]$Path
    )

    if (-not [System.IO.Path]::IsPathRooted($Path)) {
        $Path = Join-Path $PWD $Path
        Write-GrayLog ('Converting to absolute path: ' + $Path)
    }
    return $Path
}

function Uninstall-dbt {
    param(
        [string]$Location,
        [string]$ExplicitBinaryPath = ''
    )

    $binaryPath = if ($ExplicitBinaryPath) { $ExplicitBinaryPath } else { "$Location\dbt.exe" }

    if (-not (Test-Path $binaryPath)) {
        Write-ErrorAndExit ('dbt not found at: ' + $binaryPath)
    }

    # Check if file is still locked
    if (Test-FileLock -Path $binaryPath) {
        Write-ErrorAndExit 'dbt is currently running. Please close all dbt processes and try again.'
    }

    # Try to remove the file
    try {
        Remove-Item -Path $binaryPath -Force -ErrorAction Stop
    }
    catch {
        Write-ErrorAndExit ("Failed to uninstall dbt: " + $_.Exception.Message)
    }

    # Other packages' libraries and ~/.dbt stay.
    $binDir = Split-Path -Parent $binaryPath
    $libDir = Join-Path (Split-Path -Parent $binDir) 'lib\dbt'
    if ((Split-Path -Leaf $binDir) -eq 'bin' -and (Test-Path $libDir)) {
        try {
            Get-ChildItem -Path $libDir -File -Filter '*adbc_driver_bridge.*' | Remove-Item -Force -ErrorAction Stop
            if (-not (Get-ChildItem -Path $libDir -Force)) {
                Remove-Item -Path $libDir -Force -ErrorAction Stop
            }
        }
        catch {
            Write-ErrorAndExit ("Failed to remove dbt libraries: " + $_.Exception.Message)
        }
    }

    # Show a clear completion message
    Write-Host
    Write-Host
    Write-Host 'Successfully removed dbt.' -ForegroundColor Green
    Write-Host
}
#endregion

function main {
    # Set strict error handling
    $ErrorActionPreference = 'Stop'

    # Check required commands
    Test-RequiredCommands

    if ($Prefix -and $installLocation) {
        Write-ErrorAndExit '-Prefix and -installLocation cannot be combined'
    }
    if ($installLocation) {
        Write-Log 'WARNING: -installLocation is deprecated. Use -Prefix <prefix> instead to remove <prefix>\bin\dbt.exe and its drivers in <prefix>\lib\dbt.'
    } else {
        $installPrefix = if ($Prefix) { $Prefix.TrimEnd('\', '/') } else { Join-Path -Path $env:USERPROFILE -ChildPath '.local' }
        $installLocation = Join-Path -Path $installPrefix -ChildPath 'bin'
    }

    # Convert relative path to absolute if needed
    $installLocation = Set-InstallLocation -Path $installLocation

    # Uninstall dbt
    Uninstall-dbt -Location $installLocation -ExplicitBinaryPath $BinaryPath

    return
}

# Handle Ctrl+C
trap {
    Write-GrayLog 'Cancelling uninstallation...'
    if ($scriptPath -and (Test-Path $scriptPath)) {
        Remove-Item -Path $scriptPath -Force -ErrorAction SilentlyContinue
    }
    return
}

try {
    main
}
catch {
    if (-not $_.Exception.Message.StartsWith('ERROR ')) {
        Write-GrayLog 'Cancelling uninstallation...'
        Write-GrayLog $_.Exception.Message
    }
    Remove-Item -Path $scriptPath -Force -ErrorAction SilentlyContinue
}
