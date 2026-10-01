{% macro validate_merge_config(config, target_relation=none) %}
  {%- set errors = [] -%}

  {%- set base_configuration_fields = {
    'merge_update_condition': 'string',
    'merge_insert_condition': 'string',
    'merge_on_using_columns': 'sequence',
    'merge_update_columns': 'sequence',
    'merge_update_set_expressions': 'mapping',
    'merge_exclude_columns': 'sequence',
    'merge_returning_columns': 'sequence'
  } -%}

  {%- for field_name, field_type in base_configuration_fields.items() -%}
    {%- set field_value = config.get(field_name) -%}
    {%- if field_type == 'string' -%}
      {%- do validate_string_field(field_value, field_name, errors) -%}
    {%- elif field_type == 'sequence' -%}
      {%- do validate_string_list_field(field_value, field_name, errors) -%}
    {%- elif field_type == 'mapping' -%}
      {%- do validate_dict_field(field_value, field_name, errors) -%}
    {%- endif -%}
  {%- endfor -%}

  {%- do validate_ducklake_restrictions(config, target_relation, errors) -%}

  {%- do validate_merge_clauses(config, base_configuration_fields, errors) -%}

  {%- if errors -%}
    {{ exceptions.raise_compiler_error("MERGE configuration errors:\n" ~ errors|join('\n')) }}
  {%- endif -%}
{% endmacro %}


{%- macro validate_merge_clauses(config, base_configuration_fields, errors) -%}
  {%- if config.get('merge_clauses') is not none -%}
    {%- if config.get('merge_clauses') is not mapping -%}
      {%- do errors.append("merge_clauses must be a dictionary, found: " ~ config.get('merge_clauses')) -%}
    {%- else -%}
      {%- set merge_clauses = config.get('merge_clauses') -%}
      {%- set clause_types = ['when_matched', 'when_not_matched'] -%}

      {%- set has_when_matched = 'when_matched' in merge_clauses -%}
      {%- set has_when_not_matched = 'when_not_matched' in merge_clauses -%}

      {%- if not has_when_matched and not has_when_not_matched -%}
        {%- do errors.append("merge_clauses must contain at least one of 'when_matched' or 'when_not_matched' keys") -%}
      {%- endif -%}

      {%- for clause_type in clause_types -%}
        {%- if clause_type in merge_clauses -%}
          {%- do validate_merge_clause_list(merge_clauses, clause_type, errors) -%}
        {%- endif -%}
      {%- endfor -%}

      {%- set conflicting_configs = [] -%}
      {%- for config_name, config_type in base_configuration_fields.items() -%}
        {%- if config_name not in ['merge_on_using_columns', 'merge_returning_columns'] -%}
          {%- set config_value = config.get(config_name) -%}
          {%- if config_value is not none -%}
            {%- if config_type == 'sequence' -%}
              {%- if config_value|length > 0 -%}
                {%- do conflicting_configs.append(config_name) -%}
              {%- endif -%}
            {%- elif config_type == 'mapping' -%}
              {%- if config_value.keys()|length > 0 -%}
                {%- do conflicting_configs.append(config_name) -%}
              {%- endif -%}
            {%- else -%}
              {%- do conflicting_configs.append(config_name) -%}
            {%- endif -%}
          {%- endif -%}
        {%- endif -%}
      {%- endfor -%}

      {%- if conflicting_configs|length > 0 -%}
        {%- do errors.append("When merge_clauses is specified, the following basic merge configurations will be ignored and should be removed: " ~ conflicting_configs|join(', ') ~ ". Define your merge behavior within merge_clauses instead.") -%}
      {%- endif -%}
    {%- endif -%}
  {%- endif -%}
{%- endmacro -%}

{%- macro validate_merge_clause_list(merge_clauses, clause_type, errors) -%}
  {%- if merge_clauses.get(clause_type) is not sequence or merge_clauses.get(clause_type) is mapping or merge_clauses.get(clause_type) is string -%}
    {%- do errors.append("merge_clauses." ~ clause_type ~ " must be a list") -%}
  {%- elif merge_clauses.get(clause_type)|length == 0 -%}
    {%- do errors.append("merge_clauses." ~ clause_type ~ " must contain at least one element") -%}
  {%- else -%}
    {%- for clause in merge_clauses.get(clause_type) -%}
      {%- if clause is not mapping -%}
        {%- do errors.append("merge_clauses." ~ clause_type ~ " elements must be dictionaries, found: " ~ clause) -%}
      {%- endif -%}
    {%- endfor -%}
  {%- endif -%}
{%- endmacro -%}

{%- macro validate_ducklake_restrictions(config, target_relation, errors) -%}
  {%- if target_relation and adapter.is_ducklake(target_relation) -%}
    {%- set merge_clauses = config.get('merge_clauses', {}) -%}
    {%- if merge_clauses and 'when_matched' in merge_clauses -%}
      {%- set when_matched_clauses = merge_clauses.get('when_matched', []) -%}
      {%- set update_delete_count = 0 -%}

      {%- for clause in when_matched_clauses -%}
        {%- if clause is mapping and clause.get('action') in ['update', 'delete'] -%}
          {%- set update_delete_count = update_delete_count + 1 -%}
        {%- endif -%}
      {%- endfor -%}

      {%- if update_delete_count > 1 -%}
        {%- do errors.append("DuckLake MERGE restrictions: when_matched clauses can contain only a single UPDATE or DELETE action. Found " ~ update_delete_count ~ " UPDATE/DELETE actions. DuckLake currently supports only one UPDATE or DELETE operation per MERGE statement.") -%}
      {%- endif -%}
    {%- endif -%}
  {%- endif -%}
{%- endmacro -%}
