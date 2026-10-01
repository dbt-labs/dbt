{% materialization table, adapter='bigquery', supported_languages=['sql', 'python']-%}

  {%- set language = model['language'] -%}
  {%- set identifier = model['alias'] -%}
  {%- set old_relation = adapter.get_relation(database=database, schema=schema, identifier=identifier) -%}
  {%- set exists_not_as_table = (old_relation is not none and not old_relation.is_table) -%}
  {%- set target_relation = api.Relation.create(database=database, schema=schema, identifier=identifier, type='table') -%}

  -- grab current tables grants config for comparision later on
  {%- set grant_config = config.get('grants') -%}

  {{ run_hooks(pre_hooks) }}

  {#
      We only need to drop this thing if it is not a table.
      If it _is_ already a table, then we can overwrite it without downtime
      Unlike table -> view, no need for `--full-refresh`: dropping a view is no big deal
  #}
  {%- if exists_not_as_table -%}
      {{ adapter.drop_relation(old_relation) }}
  {%- endif -%}

  -- build model
  {%- set raw_partition_by = config.get('partition_by', none) -%}
  {%- set partition_by = adapter.parse_partition_by(raw_partition_by) -%}
  {%- set cluster_by = config.get('cluster_by', none) -%}
  {% if not adapter.is_replaceable(old_relation, partition_by, cluster_by) %}
    {% do log("Hard refreshing " ~ old_relation ~ " because it is not replaceable") %}
    {% do adapter.drop_relation(old_relation) %}
  {% endif %}

  -- build model
  {%- call statement('main', language=language) -%}
    {{ create_table_as(False, target_relation, compiled_code, language) }}
  {%- endcall -%}

  {{ run_hooks(post_hooks) }}

  {% set should_revoke = should_revoke(old_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}

{% macro py_write_table(compiled_code, target_relation) %}
from pyspark.sql import SparkSession
{%- set raw_partition_by = config.get('partition_by', none) -%}
{%- set raw_cluster_by = config.get('cluster_by', none) -%}
{%- set enable_list_inference = config.get('enable_list_inference', true) -%}
{%- set intermediate_format = config.get('intermediate_format', none) -%}

{%- set partition_config = adapter.parse_partition_by(raw_partition_by) %}

spark = SparkSession.builder.appName('smallTest').getOrCreate()

spark.conf.set("viewsEnabled","true")
spark.conf.set("temporaryGcsBucket","{{target.gcs_bucket}}")
spark.conf.set("enableListInference", "{{ enable_list_inference }}")
{% if intermediate_format %}
spark.conf.set("intermediateFormat", "{{ intermediate_format }}")
{% endif %}

{{ compiled_code }}
dbt = dbtObj(spark.read.format("bigquery").load)
df = model(dbt, spark)

# COMMAND ----------
# this is materialization code dbt generated, please do not modify

import pyspark
# make sure pandas exists before using it
try:
  import pandas
  pandas_available = True
except ImportError:
  pandas_available = False

# make sure pyspark.pandas exists before using it
try:
  import pyspark.pandas
  pyspark_pandas_api_available = True
except ImportError:
  pyspark_pandas_api_available = False

# make sure databricks.koalas exists before using it
try:
  import databricks.koalas
  koalas_available = True
except ImportError:
  koalas_available = False

# preferentially convert pandas DataFrames to pandas-on-Spark or Koalas DataFrames first
# since they know how to convert pandas DataFrames better than `spark.createDataFrame(df)`
# and converting from pandas-on-Spark to Spark DataFrame has no overhead
if pyspark_pandas_api_available and pandas_available and isinstance(df, pandas.core.frame.DataFrame):
  df = pyspark.pandas.frame.DataFrame(df)
elif koalas_available and pandas_available and isinstance(df, pandas.core.frame.DataFrame):
  df = databricks.koalas.frame.DataFrame(df)

# convert to pyspark.sql.dataframe.DataFrame
if isinstance(df, pyspark.sql.dataframe.DataFrame):
  pass  # since it is already a Spark DataFrame
elif pyspark_pandas_api_available and isinstance(df, pyspark.pandas.frame.DataFrame):
  df = df.to_spark()
elif koalas_available and isinstance(df, databricks.koalas.frame.DataFrame):
  df = df.to_spark()
elif pandas_available and isinstance(df, pandas.core.frame.DataFrame):
  df = spark.createDataFrame(df)
else:
  msg = f"{type(df)} is not a supported type for dbt Python materialization"
  raise Exception(msg)

# For writeMethod we need to use "indirect" if materializing a partitioned table
# otherwise we can use "direct". Note that indirect will fail if the GCS bucket has a retention policy set on it.
{%- if partition_config %}
      {%- set write_method = 'indirect' -%}
{%- else %}
      {% set write_method = 'direct' -%}
{%- endif %}

df.write \
  .mode("overwrite") \
  .format("bigquery") \
  .option("writeMethod", "{{ write_method }}") \
  .option("writeDisposition", 'WRITE_TRUNCATE') \
  {%- if partition_config is not none %}
  {%- if partition_config.data_type | lower in ('date','timestamp','datetime') %}
  .option("partitionField", "{{- partition_config.field -}}") \
  {%- if partition_config.granularity is not none %}
  .option("partitionType", "{{- partition_config.granularity| upper -}}") \
  {%- endif %}
  {%- endif %}
  {%- endif %}
  {%- if raw_cluster_by is not none %}
  .option("clusteredFields", "{{- raw_cluster_by | join(',') -}}") \
  {%- endif %}
  .save("{{target_relation}}")
{% endmacro %}

{% macro bigframes_write_table(compiled_code, target_relation) %}
import bigframes.pandas as bpd
bpd.options.compute.extra_query_labels["bigframes-dbt-api"] = "python-model-table"
bpd.options.bigquery.application_name = f"dbt-bigquery-{{dbt_version}}"
bpd.options.bigquery.ordering_mode = "partial"
bpd.options.bigquery.project = "{{ target.project }}"
{% if target.location %}
bpd.options.bigquery.location = "{{ target.location }}"
{% endif %}
session = bpd.get_global_session()
{{ compiled_code }}
dbt = dbtObj(bpd.read_gbq)
df = model(dbt, session)
df.to_gbq("{{ target_relation }}", if_exists="replace")
df._session.close()
{% endmacro %}
