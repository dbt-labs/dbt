#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
cd $SCRIPT_DIR/..

NEW_VERSION="${1}"

echo "Bumping version: ${NEW_VERSION}"
perl -pi -e "s/\bminijinja v.*? /minijinja v$NEW_VERSION /" README.md
perl -pi -e "s/^version = \".*?\"/version = \"$NEW_VERSION\"/" minijinja-py/pyproject.toml
perl -pi -e "s/^version = \".*?\"/version = \"$NEW_VERSION\"/" minijinja/Cargo.toml
perl -pi -e "s/^version = \".*?\"/version = \"$NEW_VERSION\"/" minijinja-*/Cargo.toml
perl -pi -e "s/^(minijinja.*?)version = \".*?\"/\$1version = \"$NEW_VERSION\"/" examples/*/Cargo.toml
perl -pi -e "s/^(minijinja.*?)version = \".*?\"/\$1version = \"$NEW_VERSION\"/" minijinja-*/Cargo.toml
perl -pi -e "s/^(minijinja.*?)version = \".*?\"/\$1version = \"=$NEW_VERSION\"/" minijinja-cli/Cargo.toml

cargo check --all
