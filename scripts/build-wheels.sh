#!/bin/bash -eo pipefail

DBT_PATH="$( cd "$(dirname "$0")/.." ; pwd -P )"

PYTHON_BIN=${PYTHON_BIN:-python}

echo $SCRIPTPATH
echo $PYTHON_BIN

set -x

rm -rf "$DBT_PATH"/dist
mkdir -p "$DBT_PATH"/dist

for SUBPATH in core plugins/postgres plugins/redshift plugins/bigquery plugins/snowflake
do
    rm -rf "$DBT_PATH"/"$SUBPATH"/dist
    cd "$DBT_PATH"/"$SUBPATH"
    $PYTHON_BIN setup.py sdist bdist_wheel
    cp -r "$DBT_PATH"/"$SUBPATH"/dist/* "$DBT_PATH"/dist/
done

cd "$DBT_PATH"
$PYTHON_BIN setup.py sdist bdist_wheel

set +x
