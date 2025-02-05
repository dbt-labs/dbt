# Clients README

### Jinja

#### How are materializations defined

Model materializations are defined by adapters. See the [dbt-adapters](https://github.com/dbt-labs/dbt-adapters) project for [base implementations](https://github.com/dbt-labs/dbt-adapters/tree/main/dbt/include/global_project/macros/materializations/models). Materializations are defined using syntax that isn't part of the Jinja standard library. These tags are referenced internally, and materializations can be overridden in user projects when users have specific needs.

```
-- Pseudocode for arguments
{% materialization <name>, <target name := one_of{default, adapter}> %}'
…
{% endmaterialization %}
```

These blocks are referred to Jinja extensions. Extensions are defined as part of the accepted Jinja code encapsulated within a dbt project. This includes system code used internally by dbt and user space (i.e. user-defined) macros. Extensions exist to help Jinja users create reusable code blocks or abstract objects--for us, materializations are a great use-case since we pass these around as arguments within dbt system code.

The code that defines this extension is a class `MaterializationExtension` and a `parse` routine. That code lives in [clients/jinja.py](https://github.com/dbt-labs/dbt-core/blob/main/core/dbt/clients/jinja.py). The routine
enables Jinja to parse (i.e. recognize) the unique comma separated arg structure our `materialization` tags exhibit (the `table, default` as seen above).
