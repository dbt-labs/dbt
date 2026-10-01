#!/bin/sh

# Exit on any error
set -e
set -x

echo "Running dbt commands..."
# Run dbt deps with error tracing
dbt deps \
    --log-level-file=debug \
    --log-path=/tmp/dbt.log \
    --debug \
    --fail-fast &
DBT_DEPS_PID=$!
wait $DBT_DEPS_PID || {
    echo "dbt deps failed with status $?"
    find /tmp/dbt.log -type f -name "*.log" -exec cat {} \;
    exit 1
}

# If output directory exists, remove it
if [ -d /output/target ]; then
  rm -rf /output/target
  echo "Removed existing /output/target directory."
  mkdir -p /output/target
fi

echo "Compiling dbt project..."
# Run dbt compile with error tracing
dbt compile \
    --target-path=/output/target \
    --profiles-dir=/creds \
    --log-level-file=debug \
    --log-path=/tmp/dbt.log \
    --debug \
    --fail-fast &
DBT_COMPILE_PID=$!
wait $DBT_COMPILE_PID || {
    echo "dbt compile failed with status $?"
    find /tmp/dbt.log -type f -name "*.log" -exec cat {} \;
    exit 1
}

# Print final logs even if successful
find /tmp/dbt.log -type f -name "*.log" -exec cat {} \;