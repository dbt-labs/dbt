#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
cd $SCRIPT_DIR/..

cargo publish -p minijinja
cargo publish -p minijinja-autoreload
cargo publish -p minijinja-embed
cargo publish -p minijinja-contrib
cargo publish -p minijinja-cli
