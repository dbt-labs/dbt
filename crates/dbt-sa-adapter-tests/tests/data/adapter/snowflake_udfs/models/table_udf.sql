SELECT * FROM TABLE(split_string('foo, bar, baz'))
UNION ALL
SELECT * FROM TABLE({{target.schema }}.split_string('foo, bar, baz'))
UNION ALL
SELECT * FROM TABLE({{ target.database }}.{{target.schema }}.split_string('foo, bar, baz'))
