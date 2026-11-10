-- funcsign: (relation, string) -> string
{%- macro snowflake__get_rename_interactive_table_sql(relation, new_name) -%}
    /*
    Rename or move an interactive table to the new name.

    Args:
        relation: SnowflakeRelation - interactive table relation to be renamed
        new_name: Union[str, SnowflakeRelation] - new name for `relation`
            if providing a string, the default database/schema will be used if that string is just an identifier
            if providing a SnowflakeRelation, `render` will be used to produce a fully qualified name
    Returns: templated string
    */
    alter table {{ relation }} rename to {{ new_name }}
{%- endmacro -%}
