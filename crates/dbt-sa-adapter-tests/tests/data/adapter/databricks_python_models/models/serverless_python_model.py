def model(dbt, session):
    source_df = dbt.ref('source_data')

    return source_df.filter(source_df.category == 'electronics')
