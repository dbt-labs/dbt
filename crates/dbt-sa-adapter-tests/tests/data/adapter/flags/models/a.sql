{{ print('global flags') }}
{{ print(flags['list_relations_page_limit']) }}
{{ print(flags.list_relations_page_limit) }}
{{ print(flags.get('list_relations_page_limit', 10)) }}
{{ print(flags.get('non_existent_flag', 'default_value')) }}

select 1 as id