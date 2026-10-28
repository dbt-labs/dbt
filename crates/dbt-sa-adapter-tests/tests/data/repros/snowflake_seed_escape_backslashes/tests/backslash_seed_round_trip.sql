{# Singular data test for dbt-fusion#1614.

   Asserts that every byte from `seeds/backslash_seed.csv` round-trips
   verbatim through Fusion's seed materialization → Snowflake → SELECT.

   We use Snowflake dollar-quoted string constants ($$...$$) for the
   expected values. Per Snowflake docs, dollar-quoted constants do NOT
   process backslash escapes, so this test is itself immune to the bug
   it is verifying — we are not relying on `\\` working correctly in the
   assertion just to test that `\\` works correctly in the seed.

   https://docs.snowflake.com/en/sql-reference/data-types-text#dollar-quoted-string-constants

   Failure modes detected (any non-zero result row fails the test):
     * `value_mismatch` — actual byte sequence differs from expected.
     * `missing_row`    — an expected id is absent from the seed table
                          (e.g. the seed didn't load at all, or the row
                          was silently dropped). Without this, an empty
                          `backslash_seed` would trivially pass via the
                          inner join collapsing to zero rows.
     * `unexpected_row` — the seed has an id we didn't author. Catches
                          accidental row duplication / schema drift.

   Each failing row exposes the actual + expected value AND their hex
   encodings so character-level differences (lone `\n` vs CRLF, dropped
   backslash, etc.) are unambiguous in the recorded result batch.
#}
with expected as (
    select 1  as id, $$MomBaby$$       as expected_value union all
    select 2,        $$Mom\Baby$$              union all
    select 3,        $$Mom\\Baby$$             union all
    select 4,        $$Baby\$$                 union all
    select 5,        $$line1\nline2$$          union all
    -- id 6: an actual LF inside the dollar-quoted literal.
    select 6,        $$line1
line2$$                                          union all
    select 7,        $$col1\tcol2$$            union all
    -- id 8: an actual TAB character (between col1 and col2).
    select 8,        $$col1	col2$$           union all
    select 9,        $$line1\rline2$$          union all
    select 10,       $$a\bc$$                  union all
    select 11,       $$a\fc$$                  union all
    select 12,       $$it's$$
),

actual as (
    select id, test_case, value
    from {{ ref('backslash_seed') }}
),

joined as (
    select
        coalesce(a.id, e.id)              as id,
        a.test_case                       as test_case,
        a.value                           as actual_value,
        e.expected_value                  as expected_value,
        case
            when a.id is null then 'missing_row'
            when e.id is null then 'unexpected_row'
            when a.value is distinct from e.expected_value then 'value_mismatch'
        end                               as failure_reason
    from actual a
    full outer join expected e
        on a.id = e.id
)

select
    id,
    test_case,
    failure_reason,
    actual_value,
    expected_value,
    hex_encode(to_binary(actual_value,   'utf-8')) as actual_hex,
    hex_encode(to_binary(expected_value, 'utf-8')) as expected_hex
from joined
where failure_reason is not null
order by id, failure_reason
