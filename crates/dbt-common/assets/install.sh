#!/bin/sh

# Exit immediately if a command exits with a non-zero status.
# Treat unset variables as an error when substituting.
set -eu

# Function to be called on script exit for cleanup purposes.
cleanup() {
    # Clean up any temporary directories
    if [ -n "${td:-}" ]; then
        rm -rf "$td"
    fi
    # Remove any in-progress staging binary left behind by a failed install
    if [ -n "${tmp_bin:-}" ]; then
        rm -f "$tmp_bin"
    fi
}

# Register the cleanup function to be called on EXIT.
trap cleanup EXIT

# Color codes
GRAY='\033[0;90m'
RED='\033[0;31m'
NC='\033[0m' # No Color

TARGET_VERSION=""
DBT_CDN_URL="${DBT_CDN_URL:-https://public.cdn.getdbt.com/fs}"
DBT_DB_RUNNER_BASE_URL="${DBT_DB_RUNNER_BASE_URL:-$DBT_CDN_URL/cli}"

log() {
    printf "install.sh: %s\n" "$1"
}

log_grey() {
    printf "${GRAY}install.sh: %s${NC}\n" "$1"
}

# debug messages only show where there's an error in the installation
# when the install/update is triggered from dbt system update
log_debug() {
    printf "${GRAY}install.sh: %s${NC}\n" "$1" 2>&1
}

err_and_exit() {
    Message="${1:-}"
    AdditionalInfo="${2:-}"

    info_suffix=""
    if [ -n "$AdditionalInfo" ]; then
        info_suffix=" - $AdditionalInfo"
    fi

    printf "${RED}ERROR: %s%s ${NC}\n" "$Message" "$info_suffix"
    exit 1
}

need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        err_and_exit "need $1 (command not found)"
    fi
}

# Function to install jq automatically
install_jq() {
    jq_version="1.8.1"
    jq_url=""
    jq_binary_name="jq"
    temp_dir=""
    
    log_grey "jq not found, installing automatically..."
    
    # Detect platform for jq download
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m)
    
    case "$os" in
        linux)
            case "$arch" in
                x86_64)
                    jq_url="https://github.com/jqlang/jq/releases/download/jq-${jq_version}/jq-linux-amd64"
                    ;;
                aarch64|arm64)
                    jq_url="https://github.com/jqlang/jq/releases/download/jq-${jq_version}/jq-linux-arm64"
                    ;;
                *)
                    err_and_exit "Unsupported architecture for automatic jq installation: $arch" "Please install jq manually and re-run this script"
                    ;;
            esac
            ;;
        darwin)
            case "$arch" in
                x86_64)
                    jq_url="https://github.com/jqlang/jq/releases/download/jq-${jq_version}/jq-macos-amd64"
                    ;;
                arm64)
                    jq_url="https://github.com/jqlang/jq/releases/download/jq-${jq_version}/jq-macos-arm64"
                    ;;
                *)
                    err_and_exit "Unsupported architecture for automatic jq installation: $arch" "Please install jq manually and re-run this script"
                    ;;
            esac
            ;;
        *)
            err_and_exit "Unsupported OS for automatic jq installation: $os" "Please install jq manually and re-run this script"
            ;;
    esac

    # Create temporary directory for jq installation
    temp_dir=$(mktemp -d || mktemp -d -t tmp)

    # Download jq
    log_debug "Downloading jq from: $jq_url"
    if ! curl -sL -f -o "$temp_dir/jq" "$jq_url"; then
        rm -rf "$temp_dir"
        err_and_exit "Failed to download jq. Please install jq manually and re-run this script"
    fi

    # Make jq executable
    chmod +x "$temp_dir/jq"

    # Determine where to install jq
    jq_install_dir=""
    if [ -w "/usr/local/bin" ] 2>/dev/null; then
        jq_install_dir="/usr/local/bin"
    elif [ -d "$HOME/.local/bin" ]; then
        jq_install_dir="$HOME/.local/bin"
        # Ensure it's in PATH for this session
        PATH=$PATH:$HOME/.local/bin
        export PATH
    else
        mkdir -p "$HOME/.local/bin"
        jq_install_dir="$HOME/.local/bin"
        PATH=$PATH:$HOME/.local/bin
        export PATH
    fi
    
    # Install jq
    if ! cp "$temp_dir/jq" "$jq_install_dir/jq"; then
        rm -rf "$temp_dir"
        err_and_exit "Failed to install jq to $jq_install_dir. Please install jq manually and re-run this script"
    fi
    
    rm -rf "$temp_dir"
    log_debug "Successfully installed jq to $jq_install_dir/jq"
    
    # Verify installation
    if ! command -v jq >/dev/null 2>&1; then
        err_and_exit "jq installed but not found in PATH. You may need to restart your terminal or update your PATH"
    fi
    
    return 0
}

help() {
    echo "Usage: install.sh [options]"
    echo ""
    echo "Options:"
    echo "  --update, -u       Update to latest or specified version"
    echo "  --version VER      Install version VER"
    echo "  --target TARGET    Install for target platform TARGET"
    echo "  --to DEST          Install to DEST"
    echo "  --help, -h         Show this help text"
    echo "  --package PACKAGE  Select package set [dbt|all] (all adds dbt-db-runner)"
}

update=false
while [ $# -gt 0 ]; do
    case $1 in
    --update | -u)
        update=true
        ;;
    --help | -h)
        help
        exit 0
        ;;
    --version)
        version=$2
        shift
        ;;
    --package | -p)
        package=$2
        shift
        ;;
    --target)
        target=$2
        shift
        ;;
    --to)
        dest=$2
        shift
        ;;
    *) ;;

    esac
    shift
done

# Check for jq and install if missing
check_jq() {
    if ! command -v jq >/dev/null 2>&1; then
        if ! install_jq; then
            err_and_exit "jq is required but could not be installed automatically. Please install jq manually and re-run this script."
        fi
    fi
}

check_dependencies() {
    version_arg="${1:-}"
    target_arg="${2:-}"

    # Dependencies
    need basename
    need curl
    need install
    need mkdir
    need mktemp
    need tar

    # optional dependencies
    if [ -z "$version_arg" ] || [ -z "$target_arg" ]; then
        need cut
    fi

    if [ -z "$version_arg" ]; then
        need rev
    fi

    if [ -z "$target_arg" ]; then
        need grep
    fi

    if [ -z "${dest:-}" ]; then
        dest="$HOME/.local/bin"
    else
        # Convert relative path to absolute
        case "$dest" in
            /*) ;; # Already absolute path
            *) dest="$PWD/$dest" ;; # Convert relative to absolute
        esac
    fi
}

# Check version of an installed binary
# Usage: check_binary_version binary_path binary_name
# Returns: version string if found, empty string if not found
check_binary_version() {
    binary_path="$1"
    binary_name="$2"
    version=""

    if [ -f "$binary_path" ] && [ -x "$binary_path" ]; then
        version=$("$binary_path" --version 2>/dev/null | cut -d ' ' -f 2 || echo "")
    fi

    echo "$version"
}

# Compare installed version with target version
# Usage: compare_versions current_version target_version requested_version package
# Returns: 0 if versions match, 1 if they don't match
compare_versions() {
    compare_current_version="$1"
    compare_target_version="$2"
    compare_requested_version="$3"
    compare_package="$4"

    if [ "$compare_current_version" = "$compare_target_version" ]; then
        if [ -z "$compare_requested_version" ]; then
            log "Latest $compare_package version $compare_target_version is already installed"
        else
            log "$compare_package version $compare_target_version is already installed"
        fi
        return 0
    fi

    return 1
}

# validate if a version exists on the CDN
# Usage: version_exists version target
# Returns: 0 if version exists, error if it doesn't
check_version_exists() {
    version="$1"
    target="$2"
    url="$DBT_CDN_URL/cli/fs-v$version-$target.tar.gz"
    log_debug "Checking if version $version exists on CDN: $url"
    if ! curl -sLI -f "$url" >/dev/null 2>&1; then
        err_and_exit "Version $version not found on CDN. Please check available versions and try again."
    fi
    return 0
}

# Detect and set target platform
# Usage: detect_target
# Returns: target platform string
detect_target_platform() {
    cpu_arch_target=$(uname -m)
    operating_system=$(uname -s | tr '[:upper:]' '[:lower:]')
    target=""

    if [ "$operating_system" = "linux" ]; then
        if [ "$cpu_arch_target" = "arm64" ] || [ "$cpu_arch_target" = "aarch64" ]; then
            target="aarch64-unknown-linux-gnu"
        elif [ "$cpu_arch_target" = "x86_64" ]; then
            target="x86_64-unknown-linux-gnu"
        else
            err_and_exit "Unsupported CPU Architecture: $cpu_arch_target"
        fi
    elif [ "$operating_system" = "darwin" ]; then
        if [ "$cpu_arch_target" = "arm64" ]; then
            target="aarch64-apple-darwin"
        else
            target="x86_64-apple-darwin"
        fi
    else
        err_and_exit "Unsupported OS: $operating_system"
    fi

    echo "$target"
}

show_path_instructions() {
    log_grey ""
    log_grey "NOTE: $dest may not be in your PATH."
    log_grey "To add it permanently, run one of these commands depending on your shell:"
    log_grey "  For bash/zsh: echo 'export PATH=\"\$PATH:$dest\"' >> ~/.bashrc  # or ~/.zshrc"
    log_grey ""
    log_grey "To use dbt in this session immediately, run:"
    log_grey "    export PATH=\"\$PATH:$dest\""
    log_grey ""
    log_grey "Then restart your terminal or run 'source ~/.bashrc' (or equivalent) for permanent changes"
}

# Setup shell config file and PATH
# Usage: setup_shell_config dest_path
setup_shell_config() {
    dest="$1"
    config_file=""
    shell_name=""

    # Detect shell and config file early
    if [ -n "${SHELL:-}" ]; then
        shell_name=$(basename "$SHELL")
    else
        if [ -f "$HOME/.bashrc" ]; then
            shell_name="bash"
        elif [ -f "$HOME/.profile" ]; then
            shell_name="sh"
        else
            shell_name=$(ps -p "$PPID" -o comm= | sed 's/.*\///')
        fi
    fi

    # Set config file based on shell
    if [ "$shell_name" = "zsh" ]; then
        config_file="$HOME/.zshrc"
    elif [ "$shell_name" = "bash" ]; then
        config_file="$HOME/.bashrc"
    elif [ "$shell_name" = "fish" ]; then
        config_file="$HOME/.config/fish/config.fish"
    fi

    if [ -z "$config_file" ]; then
        log_grey "NOTE: Failed to identify config file."
        show_path_instructions
        return 1
    fi

    # check if the config file exists or not and create it if it doesn't
    if [ ! -f "$config_file" ]; then
        if touch "$config_file"; then
            log_debug "Created config file $config_file"
        else
            log_grey "Note: Failed to create config file $config_file."
            show_path_instructions
            return 1
        fi
    fi

    local needs_config_path_update=false
    if ! grep -q "export PATH=\"\$PATH:$dest\"" "$config_file" 2>/dev/null; then
        needs_config_path_update=true
    fi

    local needs_path_update=false
    if ! echo "$PATH" | grep -q "$dest"; then
        needs_path_update=true
    fi

    # Check if aliases need to be updated
    local needs_alias_update=false
    if ! grep -q "alias dbtf=$dest/dbt" "$config_file" 2>/dev/null; then
        needs_alias_update=true
    fi

    if [ "$shell_name" != "fish" ]; then
        if [ "$needs_config_path_update" = true ]; then
            {
                echo "" >> "$config_file" && \
                echo "# Added by dbt installer" >> "$config_file" && \
                echo "export PATH=\"\$PATH:$dest\"" >> "$config_file" && \
                log_debug "Added $dest to PATH in $config_file"
            } || {
                log_grey "NOTE: Failed to modify $config_file."
                show_path_instructions
                return 1
            }
        fi
    else
        if [ "$needs_config_path_update" = true ]; then
            {
                echo "fish_add_path $dest" >> "$config_file"
                log_debug "Added $dest to PATH in $config_file"
            } || {
                log_grey "NOTE: Failed to modify $config_file."
                show_path_instructions
                return 1
            }
        fi
    fi

    if [ "$needs_path_update" = true ]; then
        log "To use dbt in this session, run:" && \
        log "    source $config_file" && \
        log "" && \
        log "The PATH change will be permanent for new terminal sessions."
    fi

    # Handle alias updates separately
    if [ "$needs_alias_update" = true ]; then
        {
            echo "" >> "$config_file" && \
            echo "# dbt aliases" >> "$config_file" && \
            echo "alias dbtf=$dest/dbt" >> "$config_file" && \
            log "Added alias dbtf to $config_file" && \
            log "To run with dbtf in this session, run: source $config_file"
        } || {
            log_grey "NOTE: Failed to add aliases to $config_file."
            return 1
        }
    fi

    return 0
}

# Determine version to install
# Usage: determine_version [specific_version]
# Returns: target version string
determine_version() {
    specific_version="$1"
    version_url="$DBT_CDN_URL/versions.json"
    versions=""

    TARGET_VERSION=""

    log_debug "Fetching versions from: $version_url"
    versions=$(curl -s -f "$version_url") || err_and_exit "Failed to fetch versions from $version_url"

    if [ -z "$versions" ]; then
        err_and_exit "No version information received from $version_url"
    fi

    if [ -z "$specific_version" ];then
        log_grey "Checking for latest version"
        TARGET_VERSION=$(echo "$versions" | jq -r ".latest.tag" | sed 's/^v//') || err_and_exit "Failed to parse latest version from versions data."
        log_grey "Latest available version: $TARGET_VERSION"
    # check the version that tag maps to, if the tag exists
    elif echo "$versions" | jq -e "has(\"$specific_version\")" > /dev/null;then
        log_grey "Checking for $specific_version version"
        TARGET_VERSION=$(echo "$versions" | jq -r ".\"$specific_version\".tag" | sed 's/^v//') || err_and_exit "Failed to parse requested version $specific_version from versions data."
        log_grey "$specific_version available version: $TARGET_VERSION"
    else
        TARGET_VERSION="$specific_version"
        log_grey "Requested version: $TARGET_VERSION"
    fi
}

# Display ASCII art for a package
# Usage: display_ascii_art package_name version
display_ascii_art() {
    package_name="$1"
    version="$2"

    if [ "$package_name" = "dbt" ]; then
        cat<<EOF

 =====              =====
=========        =========
 ===========    >========
  ======================         ██╗   ██╗        ██╗  
   ====================          ██║   ██║       ████╗ 
    ========--========       ██████║   ██████╗   ╚██╔╝ 
     =====-    -=====       ██╔══██║   ██╔══██╗   ██║  
    ========--========      ██║  ██║   ██║  ██║   ██║  
   ====================     ╚██████║   ██████╔╝   ╚██╗ v$version
  ======================     ╚═════╝   ╚═════╝     ╚═╝ 
 ========<   ============
=========      ==========
 =====             =====

EOF
    else
        log "Successfully installed $package_name $version to: $dest"
    fi
}

# Install a package
# Usage: install_package package_name version target dest update
# Returns: 0 on success, 1 on failure
install_package() {
    package_name="$1"
    version="$2"
    target="$3"
    dest="$4"
    update="$5"
    td=""
    url=""
    current_version=""

    # Check if already installed and get version
    current_version=$(check_binary_version "$dest/$package_name" "$package_name")
    if [ -n "$current_version" ] && [ "$current_version" = "$version" ]; then
        log "$package_name version $version is already installed"
        return 0
    elif [ -n "$current_version" ]; then
        log_debug "Current installed $package_name version: $current_version"
    fi

    # If we get here, version is different, so check if we can proceed
    if [ -e "$dest/$package_name" ] && [ "$update" = false ]; then
        err_and_exit "$package_name already exists in $dest, use the --update flag to reinstall"
    fi

    log "Installing $package_name to: $dest"
    # Create the directory if it doesn't exist
    mkdir -p "$dest"

    # Create temp directory
    td=$(mktemp -d || mktemp -d -t tmp)

    # Construct URL based on package
    case "$package_name" in
        "dbt")
            url="$DBT_CDN_URL/cli/fs-v$version-$target.tar.gz"
            ;;
        "dbt-db-runner")
            url="$DBT_DB_RUNNER_BASE_URL/fs-db-runner-v$version-$target.tar.gz"
            ;;
        *)
            err_and_exit "Invalid package name: $package_name"
            ;;
    esac

    log_debug "Downloading: $url"
    archive="$td/package.tar.gz"
    http_status=""
    if ! http_status=$(curl -sL -f -o "$archive" -w '%{http_code}' "$url"); then
        if [ "$package_name" = "dbt-db-runner" ] && [ "$http_status" = "404" ]; then
            log_grey "$package_name is not available for version $version; installing dbt without it."
            rm -f "$dest/$package_name"
            rm -rf "$td"
            td=""
            return 0
        fi
        err_and_exit "Failed to download $package_name version $version for $target."
    fi

    # Download and extract
    if ! tar -C "$td" -xzf "$archive"; then
        err_and_exit "Failed to extract package. The downloaded archive appears to be invalid."
    fi
    rm -f "$archive"

    # Check if any files were extracted
    if [ ! -d "$td" ] || [ -z "$(ls -A "$td")" ]; then
        err_and_exit "No files were extracted from the archive"
    fi

    for f in "$(cd "$td" && find . -type f)"; do
        [ -x "$td/$f" ] || {
            log_debug "File $f is not executable, skipping"
            continue
        }

        log_debug "Moving $f to $dest/$package_name"

        # Ensure the destination directory exists
        mkdir -p "$dest" || err_and_exit "Error: Failed to create installation directory: $dest"

        # Stage into a temp file in the same directory (same filesystem = atomic mv).
        # install sets permissions before the file goes live; mv -f atomically
        # replaces any existing binary without a removal gap.
        tmp_bin="$dest/.${package_name}.tmp.$$"
        install -v -m 755 "$td/$f" "$tmp_bin" 2>&1 | while IFS= read -r line; do
            log_debug "$line"
        done || err_and_exit "Error: Failed to stage $package_name binary."
        mv -f "$tmp_bin" "$dest/$package_name" || err_and_exit "Error: Failed to replace $package_name binary."
        log_debug "Installed $package_name to $dest/$package_name"
    done

    rm -rf "$td"
    td=""

    display_ascii_art "$package_name" "$version"

    if [ "$update" = true ] && [ -n "$current_version" ]; then
        log_grey "Successfully updated $package_name from $current_version to $version"
    fi

    return 0
}

# Install packages based on selection
# Usage: install_packages package target_version target dest update
# Returns: 0 on success, 1 on failure
install_packages() {
    package="$1"
    target_version="$2"
    target="$3"
    dest="$4"
    update="$5"
    current_dbt_version=""
    current_runner_version=""
    dbt_needs_update=false
    runner_needs_update=false
    install_runner=false

    if [ "$package" = "dbt-lsp" ]; then
        err_and_exit "The standalone dbt-lsp package is no longer published. Install dbt and run 'dbt lsp' instead."
    fi

    if [ "$package" != "dbt" ] && [ "$package" != "all" ]; then
        err_and_exit "Invalid package name: $package"
    fi

    # Check if versions match
    if [ "$package" = "all" ] || [ "$package" = "dbt" ]; then
        current_dbt_version=$(check_binary_version "$dest/dbt" "dbt")
        if ! compare_versions "$current_dbt_version" "$target_version" "$version" "dbt"; then
            dbt_needs_update=true
        fi
    fi

    # --package all opts in to the runner. Updates also preserve an existing
    # runner so a bundled installation stays version-matched.
    if [ "$package" = "all" ] || { [ "$update" = true ] && [ -e "$dest/dbt-db-runner" ]; }; then
        install_runner=true
        current_runner_version=$(check_binary_version "$dest/dbt-db-runner" "dbt-db-runner")
        if ! compare_versions "$current_runner_version" "$target_version" "$version" "dbt-db-runner"; then
            runner_needs_update=true
        fi
    fi

    # Exit if no updates needed
    if [ "$dbt_needs_update" = false ] && [ "$runner_needs_update" = false ]; then
        return 0
    fi

    # Install packages
    if [ "$install_runner" = true ] && [ "$runner_needs_update" = true ]; then
        if ! install_package "dbt-db-runner" "$target_version" "$target" "$dest" "$update"; then
            return 1
        fi
    fi

    if ([ "$package" = "all" ] || [ "$package" = "dbt" ]) && [ "$dbt_needs_update" = true ]; then
        if ! install_package "dbt" "$target_version" "$target" "$dest" "$update"; then
            return 1
        fi
    fi

    # Setup shell config only for dbt
    if [ "$package" = "all" ] || [ "$package" = "dbt" ]; then
        setup_shell_config "$dest"
    fi

    return 0
}

main() {
    # Initialize variables with defaults if not set by command-line parsing
    # These variables are parsed from command-line arguments (lines 155-189)
    # If not provided on command line, they will default to empty or a specific value here.
    package="${package:-dbt}"
    version="${version:-}"
    target="${target:-}"
    dest="${dest:-$HOME/.local/bin}"
    update="${update:-false}"

    if [ "$package" = "dbt-lsp" ]; then
        err_and_exit "The standalone dbt-lsp package is no longer published. Install dbt and run 'dbt lsp' instead."
    fi

    if [ "$package" != "dbt" ] && [ "$package" != "all" ]; then
        err_and_exit "Invalid package name: $package"
    fi

    check_jq

    # Determine version to install
    determine_version "$version"
    target_version="$TARGET_VERSION"

    check_dependencies "$version" "$target_version"

    target_platform="${target:-$(detect_target_platform)}"
    log_grey "Target: $target_platform"

    check_version_exists "$target_version" "$target_platform"

    install_packages "$package" "$target_version" "$target_platform" "$dest" "$update"

}


main
