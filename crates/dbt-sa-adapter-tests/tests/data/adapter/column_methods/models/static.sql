{{ print(api.Column.translate_type("timestamp")) }}
{{ print(api.Column.translate_type("string")) }}
{{ print(api.Column.translate_type("text")) }}
{{ print(api.Column.translate_type("float")) }}
{{ print(api.Column.translate_type("long")) }}
{{ print(api.Column.translate_type("integer")) }}
{{ print(api.Column.numeric_type("numeric", 28, 6)) }}
{{ print(api.Column.string_type(600)) }}
{{ print(api.Column.from_description("col", "varchar(256)")) }}

select 1 as id
