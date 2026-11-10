{% macro liquid_clustered_cols() -%}
  {%- set cols = config.get('liquid_clustered_by', validator=validation.any[list, basestring]) -%}
  {%- set auto_cluster = config.get('auto_liquid_cluster', validator=validation.any[boolean]) -%}
  {%- if cols is not none %}
    {%- if cols is string -%}
      {%- set cols = [cols] -%}
    {%- endif -%}
    CLUSTER BY ({{ cols | join(', ') }})
    {%- elif auto_cluster -%}
    CLUSTER BY AUTO
  {%- endif %}
{%- endmacro -%}

{% macro apply_liquid_clustered_cols(target_relation, liquid_clustering, existing_relation=none) -%}
  {%- set cols = liquid_clustering.cluster_by -%}
  {%- set auto_cluster = liquid_clustering.auto_cluster -%}
  {%- if cols and cols != [] %}
    {%- call statement('set_cluster_by_columns') -%}
      {#- DIVERGENCE BEGIN: upstream uses target_relation.type.render(); we use render_type() Jinja macro instead -#}
      ALTER {{ render_type(target_relation.type) }} {{ target_relation.render() }} CLUSTER BY ({{ cols | join(', ') }})
      {#- DIVERGENCE END -#}
    {%- endcall -%}
  {%- elif auto_cluster -%}
    {#-- CLUSTER BY AUTO is unsupported on shallow clones; look up the type if the caller didn't pass it. --#}
    {%- set clone_check = existing_relation if existing_relation is not none else load_relation_with_metadata(target_relation) -%}
    {%- if clone_check is not none and clone_check.is_shallow_clone -%}
      {{ exceptions.warn("Skipping CLUSTER BY AUTO on " ~ target_relation ~ " because it is a shallow clone; auto liquid clustering is only supported on Unity Catalog managed tables.") }}
    {%- else -%}
      {%- call statement('set_cluster_by_auto') -%}
        {#- DIVERGENCE BEGIN: upstream uses target_relation.type.render(); we use render_type() Jinja macro instead -#}
        ALTER {{ render_type(target_relation.type) }} {{ target_relation.render() }} CLUSTER BY AUTO
        {#- DIVERGENCE END -#}
      {%- endcall -%}
    {%- endif -%}
  {% else %}
    {%- call statement('unset_cluster_by') -%}
      {#- DIVERGENCE BEGIN: upstream uses target_relation.type.render(); we use render_type() Jinja macro instead -#}
      ALTER {{ render_type(target_relation.type) }} {{ target_relation.render() }} CLUSTER BY NONE
      {#- DIVERGENCE END -#}
    {%- endcall -%}
  {%- endif %}
{%- endmacro -%}