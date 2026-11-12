# Basic Python model with table materialization
# This tests the simplest Python model use case on Databricks

def model(dbt, session):
    """
    Simple Python model that references a SQL model.
    Uses PySpark DataFrame operations.
    """
    dbt.config(
        materialized='table',
        submission_method='job_cluster',
        create_notebook=True,
        job_cluster_config={
            'spark_version': '15.4.x-scala2.12',
            'node_type_id': 'i3.xlarge',
            'num_workers': 1,
        }
    )

    # Reference the SQL source model
    source_df = dbt.ref('source_data')

    # Simple transformation: filter and select
    result_df = source_df.filter(source_df.total_amount > 50)

    return result_df

