# Basic Python model with BigFrames submission method
# This tests the simplest Python model use case on BigQuery using BigFrames
# See: https://docs.getdbt.com/guides/dbt-python-bigframes

def model(dbt, session):
    """
    Simple Python model that references a SQL model.
    Uses BigQuery DataFrames (BigFrames) for scalable BigQuery SQL.
    """
    dbt.config(
        materialized='table',
        submission_method='bigframes'
    )
    bdf = dbt.ref("source_data") #loading from prev step
    return bdf
