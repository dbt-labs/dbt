{{ print(adapter.quote_as_configured("database_name", "database")) }}
{{ print(adapter.quote_as_configured("schema_name", "schema")) }}
{{ print(adapter.quote_as_configured("table_name", "identifier")) }}

select 1 as one
