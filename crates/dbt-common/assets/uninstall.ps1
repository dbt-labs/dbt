# Uninstall script for dbt
[CmdletBinding()]
param(
    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [ValidateScript({
        if(-not (Test-Path -Path $_ -IsValid)) {
            throw "Path '$_' contains invalid characters"
        }
        return $true
    })]
    [string]$installLocation = "$env:USERPROFILE\.local\bin",

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
