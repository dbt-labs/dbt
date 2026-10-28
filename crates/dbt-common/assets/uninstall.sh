#!/bin/sh
set -e

RED='\033[0;31m'
NC='\033[0m' # No Color

log() {
    printf "uninstall.sh: %s\n" "$1"
}

err_and_exit() {
    local Message="${1:-}"
    local AdditionalInfo="${2:-}"

    if [ ! -z "${td:-}" ]; then
        rm -rf "$td"
    fi

    local info_suffix=""
    if [ -n "$AdditionalInfo" ]; then
        info_suffix=" - $AdditionalInfo"
    fi

    printf "${RED}ERROR: %s%s ${NC}\n" "$Message" "$info_suffix"
    exit 1
}

help() {
    echo "Usage: uninstall.sh [options]"
    echo ""
    echo "Options:"
    echo "  --prefix PREFIX           Remove dbt from PREFIX/bin and its drivers from"
    echo "                            PREFIX/lib/dbt (default: ~/.local)"
    echo "  --installLocation, -i     Deprecated: the directory holding dbt. Use --prefix"
    echo "  --binaryPath              Exact path to the dbt binary to remove,"
    echo "                            overriding installLocation/dbt for the"
    echo "                            main binary only (e.g. a renamed copy)"
    echo "  --package PACKAGE         Uninstall package PACKAGE [dbt|dbt-lsp|all]"
    echo "  --help, -h                Show this help text"
}

while test $# -gt 0; do
    case $1 in
    --help | -h)
        help
        exit 0
        ;;
    --prefix)
        prefix=$2
        shift
        ;;
    --installLocation | -i)
        installLocation=$2
        shift
        ;;
    --binaryPath)
        binaryPath=$2
        shift
        ;;
    --package | -p)
        package=$2
        shift
        ;;
    *) ;;

    esac
    shift
done

package="${package:-dbt}"

# set install locations
if [ -n "${prefix:-}" ] && [ -n "${installLocation:-}" ]; then
    err_and_exit "--prefix and --installLocation cannot be combined"
fi
if [ -n "${installLocation:-}" ]; then
    log "WARNING: --installLocation is deprecated. Use --prefix PREFIX instead to remove PREFIX/bin/dbt and its drivers in PREFIX/lib/dbt."
else
    prefix="${prefix:-$HOME/.local}"
    installLocation="${prefix%/}/bin"
fi
dbtInstallLocation="$installLocation/dbt"
runnerInstallLocation="$installLocation/dbt-db-runner"
lspInstallLocation="$installLocation/dbt-lsp"

# --binaryPath overrides the main dbt binary's location only -- it may be
# named something other than "dbt" (e.g. dbtf, as the VS Code extension
# names it to avoid colliding with a separate OSS dbt on PATH). Optional and
# additive: older callers that only pass --installLocation, or nothing at
# all, keep the exact behavior above. The runner/lsp locations are
# unaffected, since those companions always keep their literal names
# regardless of what the main binary is named.
if [ -n "${binaryPath:-}" ]; then
    dbtInstallLocation="$binaryPath"
fi

# check if package is valid
if [ "$package" != "all" ] && [ "$package" != "dbt" ] && [ "$package" != "dbt-lsp" ]; then
    err_and_exit "Invalid package: $package"
fi

# check if operating system is supported
operating_system=$(uname -s | tr '[:upper:]' '[:lower:]')

if [ "$operating_system" != "linux" ] && [ "$operating_system" != "darwin" ]; then
    err_and_exit "Unsupported OS: $operating_system"
fi

# uninstall dbt / dbt-lsp
if [ "$package" = "all" ] || [ "$package" = "dbt" ]; then
    bin_dir="$(dirname "$dbtInstallLocation")"
    rm -f "$dbtInstallLocation" "$runnerInstallLocation"
    log "Uninstalled dbt from $dbtInstallLocation"
    log "Uninstalled dbt-db-runner from $runnerInstallLocation"
    # Other packages' libraries and ~/.dbt stay.
    lib_dir="$(dirname "$bin_dir")/lib/dbt"
    if [ "$(basename "$bin_dir")" = "bin" ] && [ -d "$lib_dir" ]; then
        rm -f "$lib_dir"/*adbc_driver_bridge.*
        rmdir "$lib_dir" 2>/dev/null || true
        rmdir "$(dirname "$lib_dir")" 2>/dev/null || true
        log "Removed bundled drivers from $lib_dir"
    fi
fi

if [ "$package" = "all" ] || [ "$package" = "dbt-lsp" ]; then
    rm -f "$lspInstallLocation"
    log "Uninstalled dbt-lsp from $lspInstallLocation"
fi
