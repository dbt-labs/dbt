{#- Exercise the `default__equals` macro so record/replay captures the
    adapter-specific SQL rendered by `adapter.render_equals()` under Fusion.
    See DIVERGENCE marker in dbt-adapters/macros/utils/equals.sql. -#}

select
    {{ equals('1', '1') }} as literal_eq_true,
    {{ equals('1', '2') }} as literal_eq_false,
    {{ equals('null', 'null') }} as null_eq_null,
    {{ equals('1', 'null') }} as literal_eq_null
