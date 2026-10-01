{# Exasol's MD5 hashing builtin is HASH_MD5, which returns CHAR(32) hex
   (matching dbt's cross-warehouse md5 surrogate keys). HASHTYPE_MD5 returns a
   binary HASHTYPE(16 byte), which would change the key format, so HASH_MD5 is
   the right choice here.

   The CAST to VARCHAR is required, not cosmetic: Exasol docs state the input
   data type is significant (HASH_MD5(123) != HASH_MD5('123')), so we hash the
   string form to match md5(cast(field as string)) on other warehouses. The
   2,000,000 max width is deliberate -- VARCHAR is unpadded so the width does
   not change the hash, but a narrower cast could truncate a long concatenated
   surrogate key before hashing. #}
{% macro exasol__hash(field) -%}
    hash_md5(cast({{ field }} as varchar(2000000)))
{%- endmacro %}
