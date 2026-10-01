#!/usr/bin/env bash
# Benchmark: end-to-end partial-parse performance across commands, scenarios, and selectors.
#
# Usage:
#   DBT_BENCH_PROJECT=~/tmp/scale_6k \
#   DBT_BIN=~/fs/target/release/dbt \
#   bash fs/sa/crates/dbt-metadata/benches/partial_parse_bench.sh
#
# Outputs a Markdown table to stdout and writes perf_partial_parse_results.json.
#
# Prerequisites:
#   - A release build of dbt: cargo build -p dbt-cli --release
#   - The scale_6k project (internal, requires AWS access to sdfdatasets bucket):
#       mkdir -p ~/tmp/scale_6k && cd ~/tmp/scale_6k
#       aws s3 cp s3://sdfdatasets/scale/6k_workspace.zip .
#       unzip 6k_workspace.zip && rm 6k_workspace.zip
#       # ensure compile succeeds (replace not_null with unique to avoid get_where_subquery):
#       printf 'version: 2\nmodels:\n  - name: t0\n    columns:\n      - name: t0_c0\n        tests:\n          - unique\n' > models/schema_bench.yml

set -euo pipefail

DBT_BENCH_PROJECT="${DBT_BENCH_PROJECT:-${HOME}/tmp/scale_6k}"
DBT_BIN="${DBT_BIN:-${HOME}/fs/target/release/dbt}"
REPS="${BENCH_REPS:-3}"
TARGET_DIR="${DBT_BENCH_PROJECT}/target"
CACHE="${TARGET_DIR}/parse_state"

# ── helpers ──────────────────────────────────────────────────────────────────

die() { echo "ERROR: $*" >&2; exit 1; }

[[ -d "${DBT_BENCH_PROJECT}" ]] || die "Project not found: ${DBT_BENCH_PROJECT}"
[[ -x "${DBT_BIN}" ]]           || die "dbt binary not found: ${DBT_BIN}"

# Run a dbt command N times and return the median wall time in ms.
# Usage: time_cmd <reps> [--pre-hook <shell-cmd>] <dbt args...>
#   --pre-hook  shell command run before each rep (e.g. re-touch files between reps)
time_cmd() {
    local reps="$1"; shift
    local pre_hook=""
    if [[ "${1:-}" == "--pre-hook" ]]; then
        pre_hook="$2"; shift 2
    fi
    local times=()
    for (( i=0; i<reps; i++ )); do
        [[ -n "${pre_hook}" ]] && eval "${pre_hook}"
        local t0 t1 ms rc
        t0=$(date +%s%3N)
        "${DBT_BIN}" "$@" \
            --project-dir "${DBT_BENCH_PROJECT}" \
            --profiles-dir "${DBT_BENCH_PROJECT}" \
            >/dev/null 2>&1 || true   # never abort the script on dbt failure
        t1=$(date +%s%3N)
        ms=$(( t1 - t0 ))
        times+=("${ms}")
    done
    # Sort and pick median
    local sorted
    IFS=$'\n' read -r -d '' -a sorted \
        < <(printf '%s\n' "${times[@]}" | sort -n && printf '\0') || true
    local mid=$(( reps / 2 ))
    echo "${sorted[${mid}]}"
}

# Touch N random model files. Each file is printed so the caller can capture
# the list via mapfile and pass to untouch_models / a pre-hook.
touch_models() {
    local n="$1"
    find "${DBT_BENCH_PROJECT}/models" -name "*.sql" \
        | shuf | head -n "${n}" \
        | while read -r f; do touch "$f"; echo "$f"; done
}

# Add a new model file (simulates `dbt parse` with a new file).
# Prints the path so it can be removed after the bench.
add_model() {
    local name="__bench_new_model__"
    local path="${DBT_BENCH_PROJECT}/models/${name}.sql"
    echo "select 1 as id" > "${path}"
    echo "${path}"
}

# Remove a file (to simulate deletion).
remove_model() {
    rm -f "$1"
}

# Reset file timestamps to 1 second in the past so subsequent runs see 0 changes.
untouch_models() {
    for f in "$@"; do
        touch -m -d "1 second ago" "$f"
    done
}

# Drop the parquet cache directory (forces a cold start with --partial-parse).
drop_cache() {
    rm -rf "${CACHE}"
}

# Report size of the parquet cache directory.
cache_size() {
    if [[ -d "${CACHE}" ]]; then
        du -sh "${CACHE}" 2>/dev/null | cut -f1
    else
        echo "0"
    fi
}

# ── project info ─────────────────────────────────────────────────────────────

# Ensure the cache exists for initial node counts
if [[ ! -d "${CACHE}" ]]; then
    echo "No cache found — running cold parse to populate..."
    "${DBT_BIN}" parse --partial-parse \
        --project-dir "${DBT_BENCH_PROJECT}" \
        --profiles-dir "${DBT_BENCH_PROJECT}" >/dev/null 2>&1
fi

# Pick a representative model name from the project models dir.
SINGLE_MODEL=$(find "${DBT_BENCH_PROJECT}/models" -name "*.sql" \
    | head -1 | xargs -I{} basename {} .sql 2>/dev/null || echo "")
PREFIX_GROUP="adder"   # ~19 models in scale_6k

echo "=================================================================="
echo " Partial-parse benchmark"
echo " Project    : ${DBT_BENCH_PROJECT}"
echo " Binary     : ${DBT_BIN}"
echo " Cache size : $(cache_size) (${CACHE})"
echo " Reps       : ${REPS} (median reported)"
echo "=================================================================="
echo ""

# ── warm up OS page cache ─────────────────────────────────────────────────────

echo "Warming OS page cache (1x parse --partial-parse)..."
"${DBT_BIN}" parse --partial-parse \
    --project-dir "${DBT_BENCH_PROJECT}" \
    --profiles-dir "${DBT_BENCH_PROJECT}" >/dev/null 2>&1 || true
echo ""

# ── results accumulator ───────────────────────────────────────────────────────

declare -A RESULTS

record() {
    local key="$1" val="$2"
    RESULTS["${key}"]="${val}"
    printf "  %-65s %s ms\n" "${key}" "${val}"
}

# ── SECTION 1: parse ─────────────────────────────────────────────────────────

echo "── parse ────────────────────────────────────────────────────────"

# Cold, no --partial-parse (no cache written or read)
drop_cache
record "parse  cold  no-pp" \
    "$(time_cmd 1 parse)"

# Cold, --partial-parse (cache does not exist → parse everything, write cache)
drop_cache
record "parse  cold  pp   (no cache → cold parse + write)" \
    "$(time_cmd 1 parse --partial-parse)"

# Warm, no partial-parse
record "parse  warm  no-pp" \
    "$(time_cmd "${REPS}" parse)"

# Warm, pp, 0 files changed
record "parse  warm  pp   0-files" \
    "$(time_cmd "${REPS}" parse --partial-parse)"

# Warm, pp, 1/10/100 files changed — re-touch before each rep so every rep
# sees the same changeset (dbt updates timestamps on save, so without pre-hook
# reps 2+ would see 0 changes and give a misleadingly fast reading).
mapfile -t f1 < <(touch_models 1);   TOUCH1="${f1[*]}"
record "parse  warm  pp   1-file  changed" \
    "$(time_cmd "${REPS}" --pre-hook "touch ${TOUCH1}" parse --partial-parse)"
untouch_models "${f1[@]}"

mapfile -t f10 < <(touch_models 10); TOUCH10="${f10[*]}"
record "parse  warm  pp   10-files changed" \
    "$(time_cmd "${REPS}" --pre-hook "touch ${TOUCH10}" parse --partial-parse)"
untouch_models "${f10[@]}"

mapfile -t f100 < <(touch_models 100); TOUCH100="${f100[*]}"
record "parse  warm  pp   100-files changed" \
    "$(time_cmd "${REPS}" --pre-hook "touch ${TOUCH100}" parse --partial-parse)"
untouch_models "${f100[@]}"

# Add a new file — forces a full cold parse (file set changed)
NEW_MODEL=$(add_model)
record "parse  warm  pp   +1 new file  (triggers full re-parse)" \
    "$(time_cmd 1 parse --partial-parse)"
remove_model "${NEW_MODEL}"
# Restore clean cache after file removal
"${DBT_BIN}" parse --partial-parse \
    --project-dir "${DBT_BENCH_PROJECT}" \
    --profiles-dir "${DBT_BENCH_PROJECT}" >/dev/null 2>&1 || true

echo ""

# ── SECTION 2: compile ───────────────────────────────────────────────────────

echo "── compile ──────────────────────────────────────────────────────"

drop_cache
record "compile  cold  no-pp" \
    "$(time_cmd 1 compile)"

drop_cache
record "compile  cold  pp   (no cache → cold parse + write)" \
    "$(time_cmd 1 compile --partial-parse)"

record "compile  warm  no-pp" \
    "$(time_cmd "${REPS}" compile)"

record "compile  warm  pp   0-files" \
    "$(time_cmd "${REPS}" compile --partial-parse)"

mapfile -t f1 < <(touch_models 1);   TOUCH1="${f1[*]}"
record "compile  warm  pp   1-file  changed" \
    "$(time_cmd "${REPS}" --pre-hook "touch ${TOUCH1}" compile --partial-parse)"
untouch_models "${f1[@]}"

mapfile -t f10 < <(touch_models 10); TOUCH10="${f10[*]}"
record "compile  warm  pp   10-files changed" \
    "$(time_cmd "${REPS}" --pre-hook "touch ${TOUCH10}" compile --partial-parse)"
untouch_models "${f10[@]}"

mapfile -t f100 < <(touch_models 100); TOUCH100="${f100[*]}"
record "compile  warm  pp   100-files changed" \
    "$(time_cmd "${REPS}" --pre-hook "touch ${TOUCH100}" compile --partial-parse)"
untouch_models "${f100[@]}"

NEW_MODEL=$(add_model)
record "compile  warm  pp   +1 new file  (triggers full re-parse)" \
    "$(time_cmd 1 compile --partial-parse)"
remove_model "${NEW_MODEL}"
"${DBT_BIN}" parse --partial-parse \
    --project-dir "${DBT_BENCH_PROJECT}" \
    --profiles-dir "${DBT_BENCH_PROJECT}" >/dev/null 2>&1 || true

echo ""

# ── SECTION 3: compile with selectors ────────────────────────────────────────

echo "── compile + selectors (warm, pp, 0 files changed) ─────────────"

record "compile  warm  pp   select=1-model  fqn  (0 files, index-load)" \
    "$(time_cmd "${REPS}" compile --partial-parse --select "${SINGLE_MODEL}")"

record "compile  warm  pp   select=~19-models  prefix  (0 files, full-load)" \
    "$(time_cmd "${REPS}" compile --partial-parse --select "${PREFIX_GROUP}_*")"

# Baseline: same selector without pp (no cache)
record "compile  warm  no-pp  select=1-model  fqn" \
    "$(time_cmd "${REPS}" compile --select "${SINGLE_MODEL}")"

echo ""

# ── SECTION 3b: best-case index-load scenario ─────────────────────────────────
# 1 file changed + narrow selector — the best case for index-based payload loading.
# Only the selected model + its direct deps + macros are deserialized from SQLite,
# instead of all nodes. Compare to compile warm pp 1-file (no selector) above.

echo "── compile + selectors (warm, pp, 1 file changed) ───────────────"

# Use REPS=1 here: the cache saves the new mtime after rep 1, so subsequent reps
# see 0 changes (FastReuse) and give a misleadingly fast median. Single-shot is honest.
mapfile -t touched_sel < <(touch_models 1); TOUCH_SEL="${touched_sel[*]}"
SEL_MODEL=$(basename "${touched_sel[0]}" .sql)

record "compile  warm  pp   1-file  select=that-model  (index-load, best case)" \
    "$(time_cmd 1 --pre-hook "touch ${TOUCH_SEL}" compile --partial-parse --select "${SEL_MODEL}")"
untouch_models "${touched_sel[@]}"

mapfile -t touched_sel < <(touch_models 1); TOUCH_SEL="${touched_sel[*]}"
record "compile  warm  pp   1-file  no-select           (full-load, runs all)" \
    "$(time_cmd 1 --pre-hook "touch ${TOUCH_SEL}" compile --partial-parse)"
untouch_models "${touched_sel[@]}"

echo ""

# ── SECTION 4: run ───────────────────────────────────────────────────────────

echo "── run ──────────────────────────────────────────────────────────"

drop_cache
record "run  cold  no-pp" \
    "$(time_cmd 1 run)"

drop_cache
record "run  cold  pp   (no cache → cold parse + write)" \
    "$(time_cmd 1 run --partial-parse)"

record "run  warm  no-pp" \
    "$(time_cmd "${REPS}" run)"

record "run  warm  pp   0-files" \
    "$(time_cmd "${REPS}" run --partial-parse)"

mapfile -t f1 < <(touch_models 1); TOUCH1="${f1[*]}"
record "run  warm  pp   1-file  changed" \
    "$(time_cmd "${REPS}" --pre-hook "touch ${TOUCH1}" run --partial-parse)"
untouch_models "${f1[@]}"

mapfile -t f10 < <(touch_models 10); TOUCH10="${f10[*]}"
record "run  warm  pp   10-files changed" \
    "$(time_cmd "${REPS}" --pre-hook "touch ${TOUCH10}" run --partial-parse)"
untouch_models "${f10[@]}"

record "run  warm  pp   select=1-model  fqn  (index-safe, lazy load)" \
    "$(time_cmd "${REPS}" run --partial-parse --select "${SINGLE_MODEL}")"

echo ""

# ── SECTION 5: test ──────────────────────────────────────────────────────────

HAS_TESTS=$(find "${DBT_BENCH_PROJECT}" -name "schema.yml" -o -name "schema.yaml" 2>/dev/null | head -1)
if [[ -n "${HAS_TESTS}" ]]; then
    echo "── test ─────────────────────────────────────────────────────────"

    drop_cache
    record "test  cold  no-pp" \
        "$(time_cmd 1 test)"

    drop_cache
    record "test  cold  pp   (no cache → cold parse + write)" \
        "$(time_cmd 1 test --partial-parse)"

    record "test  warm  no-pp" \
        "$(time_cmd "${REPS}" test)"

    record "test  warm  pp   0-files" \
        "$(time_cmd "${REPS}" test --partial-parse)"

    mapfile -t f1 < <(touch_models 1); TOUCH1="${f1[*]}"
    record "test  warm  pp   1-file  changed" \
        "$(time_cmd "${REPS}" --pre-hook "touch ${TOUCH1}" test --partial-parse)"
    untouch_models "${f1[@]}"

    echo ""
else
    echo "(Skipping test section — no schema.yml found. Add a schema.yml with tests.)"
    echo ""
fi

# ── SECTION 6: list ──────────────────────────────────────────────────────────

echo "── list ─────────────────────────────────────────────────────────"

record "list  warm  no-pp" \
    "$(time_cmd "${REPS}" list)"

record "list  warm  pp   0-files" \
    "$(time_cmd "${REPS}" list --partial-parse)"

record "list  warm  pp   select=1-model  fqn  (index-safe, lazy load)" \
    "$(time_cmd "${REPS}" list --partial-parse --select "${SINGLE_MODEL}")"

echo ""

# ── SECTION 7: show ──────────────────────────────────────────────────────────
# `dbt show` runs a SELECT against the adapter — on scale_6k with DuckDB-style
# profiles it typically errors unless a real adapter is configured.  We still
# benchmark it because the expensive part is parsing/loading, which always runs
# before the adapter call.  Failures are suppressed (|| true in time_cmd).

echo "── show ─────────────────────────────────────────────────────────"

record "show  warm  no-pp  select=1-model  fqn" \
    "$(time_cmd "${REPS}" show --select "${SINGLE_MODEL}")"

record "show  warm  pp   0-files  select=1-model  fqn  (lazy: model/test/analysis)" \
    "$(time_cmd "${REPS}" show --partial-parse --select "${SINGLE_MODEL}")"

record "show  warm  pp   0-files  no-select  (full load)" \
    "$(time_cmd "${REPS}" show --partial-parse)"

echo ""

# ── Markdown table ────────────────────────────────────────────────────────────

print_row() {
    local key="$1" baseline="${2:-0}"
    local val="${RESULTS[${key}]:-?}"
    local rel=""
    if [[ -n "${baseline}" && "${val}" != "?" && "${baseline}" -gt 0 ]]; then
        rel="$(awk "BEGIN{printf \"%.1fx\", ${baseline}/${val}}")"
    fi
    printf "| %-65s | %5s | %6s |\n" "${key}" "${val}" "${rel}"
}

PARSE_BASELINE="${RESULTS[parse  warm  no-pp]:-0}"
COMPILE_BASELINE="${RESULTS[compile  warm  no-pp]:-0}"
RUN_BASELINE="${RESULTS[run  warm  no-pp]:-0}"
TEST_BASELINE="${RESULTS[test  warm  no-pp]:-0}"
LIST_BASELINE="${RESULTS[list  warm  no-pp]:-0}"
SHOW_BASELINE="${RESULTS[show  warm  no-pp  select=1-model  fqn]:-0}"

echo "## Results (cache: $(cache_size), median over ${REPS} reps, OS page cache warm)"
echo ""
echo "| Scenario | ms | vs. no-pp |"
echo "|---|---:|---:|"

print_row "parse  cold  no-pp"
print_row "parse  cold  pp   (no cache → cold parse + write)"
print_row "parse  warm  no-pp"                              "${PARSE_BASELINE}"
print_row "parse  warm  pp   0-files"                       "${PARSE_BASELINE}"
print_row "parse  warm  pp   1-file  changed"               "${PARSE_BASELINE}"
print_row "parse  warm  pp   10-files changed"              "${PARSE_BASELINE}"
print_row "parse  warm  pp   100-files changed"             "${PARSE_BASELINE}"
print_row "parse  warm  pp   +1 new file  (triggers full re-parse)"
echo "| | | |"
print_row "compile  cold  no-pp"
print_row "compile  cold  pp   (no cache → cold parse + write)"
print_row "compile  warm  no-pp"                            "${COMPILE_BASELINE}"
print_row "compile  warm  pp   0-files"                     "${COMPILE_BASELINE}"
print_row "compile  warm  pp   1-file  changed"             "${COMPILE_BASELINE}"
print_row "compile  warm  pp   10-files changed"            "${COMPILE_BASELINE}"
print_row "compile  warm  pp   100-files changed"           "${COMPILE_BASELINE}"
print_row "compile  warm  pp   +1 new file  (triggers full re-parse)"
echo "| | | |"
print_row "compile  warm  pp   select=1-model  fqn  (0 files, index-load)"  "${COMPILE_BASELINE}"
print_row "compile  warm  pp   select=~19-models  prefix  (0 files, full-load)" "${COMPILE_BASELINE}"
print_row "compile  warm  no-pp  select=1-model  fqn"
echo "| | | |"
print_row "compile  warm  pp   1-file  select=that-model  (index-load, best case)" "${COMPILE_BASELINE}"
print_row "compile  warm  pp   1-file  no-select           (full-load, runs all)"  "${COMPILE_BASELINE}"
echo "| | | |"
print_row "run  cold  no-pp"
print_row "run  cold  pp   (no cache → cold parse + write)"
print_row "run  warm  no-pp"                                "${RUN_BASELINE}"
print_row "run  warm  pp   0-files"                         "${RUN_BASELINE}"
print_row "run  warm  pp   1-file  changed"                 "${RUN_BASELINE}"
print_row "run  warm  pp   10-files changed"                "${RUN_BASELINE}"
print_row "run  warm  pp   select=1-model  fqn  (index-safe, lazy load)" "${RUN_BASELINE}"
echo "| | | |"
if [[ -n "${HAS_TESTS}" ]]; then
    print_row "test  cold  no-pp"
    print_row "test  cold  pp   (no cache → cold parse + write)"
    print_row "test  warm  no-pp"                           "${TEST_BASELINE}"
    print_row "test  warm  pp   0-files"                    "${TEST_BASELINE}"
    print_row "test  warm  pp   1-file  changed"            "${TEST_BASELINE}"
    echo "| | | |"
fi
print_row "list  warm  no-pp"                               "${LIST_BASELINE}"
print_row "list  warm  pp   0-files"                        "${LIST_BASELINE}"
print_row "list  warm  pp   select=1-model  fqn  (index-safe, lazy load)" "${LIST_BASELINE}"
echo "| | | |"
print_row "show  warm  no-pp  select=1-model  fqn"          "${SHOW_BASELINE}"
print_row "show  warm  pp   0-files  select=1-model  fqn  (lazy: model/test/analysis)" "${SHOW_BASELINE}"
print_row "show  warm  pp   0-files  no-select  (full load)" "${SHOW_BASELINE}"

echo ""

# ── Write JSON ────────────────────────────────────────────────────────────────

JSON_FILE="perf_partial_parse_results.json"
{
    echo "{"
    printf '  "project": "%s",\n' "${DBT_BENCH_PROJECT}"
    printf '  "reps": %s,\n' "${REPS}"
    printf '  "cache_size": "%s",\n' "$(cache_size)"
    echo '  "results_ms": {'
    first=1
    for key in "${!RESULTS[@]}"; do
        [[ "${first}" == 1 ]] || printf ',\n'
        printf '    "%s": %s' "${key}" "${RESULTS[${key}]}"
        first=0
    done
    printf '\n  }\n'
    echo "}"
} > "${JSON_FILE}"

echo "Results written to ${JSON_FILE}"
