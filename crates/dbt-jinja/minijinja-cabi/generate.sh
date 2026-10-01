#!/usr/bin/env bash

CABI_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
header_file_backup="$CABI_DIR/include/minijinja.h.backup"

function cleanup {
  rm -rf "$WORK_DIR" || true
  rm "$header_file_backup" || true
}

trap cleanup EXIT

WORK_DIR=$(mktemp -d)

cp "$CABI_DIR/include/minijinja.h" "$header_file_backup"

if ! cargo expand 2> $WORK_DIR/expand_stderr.err > $WORK_DIR/expanded.rs; then
    cat $WORK_DIR/expand_stderr.err
fi

if ! cbindgen \
    --config "$CABI_DIR/cbindgen.toml" \
    --lockfile "$CABI_DIR/../Cargo.lock" \
    --output "$CABI_DIR/include/minijinja.h" \
    "${@}"\
    $WORK_DIR/expanded.rs 2> $WORK_DIR/cbindgen_stderr.err; then
    bindgen_exit_code=$?
    if [[ "--verify" == "$1" ]]; then
        echo "Changes from previous header (old < > new)"
        diff -u "$header_file_backup" "$CABI_DIR/include/minijinja.h"
    else
        echo "cbindgen failed:"
        cat $WORK_DIR/cbindgen_stderr.err
    fi
    exit $bindgen_exit_code
fi

