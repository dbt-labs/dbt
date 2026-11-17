"""Arrow bridge for the native adapter; no dependency on a Python dbt adapter."""
import contextlib
import json
import random
import time

import adbc_driver_manager as adbc
import sys

import adbc_driver_manager.dbapi as dbapi
import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.csv as csv


def arrow_result(result):
    if isinstance(result, (pa.Table, pa.RecordBatchReader)):
        return result
    if isinstance(result, pa.RecordBatch):
        return pa.Table.from_batches([result])
    library = type(result).__module__.split('.')[0]
    if library == 'pandas':
        return pa.Table.from_pandas(result, preserve_index=False)
    if library == 'polars':
        return (result.collect() if hasattr(result, 'collect') else result).to_arrow()
    if hasattr(result, '__arrow_c_stream__'):
        return pa.RecordBatchReader.from_stream(result)
    raise TypeError('model() must return an Arrow table/reader, pandas DataFrame, or polars DataFrame')


class Session:
    def __init__(self, connection):
        self.connection = connection

    def sql(self, sql):
        with self.connection.cursor() as cursor:
            cursor.execute(sql)
            return cursor.fetch_arrow_table()


def ingest(connection, identifier, data, schema):
    for attempt in range(20):
        try:
            with connection.cursor() as cursor:
                upload = upload_input(data)
                return cursor.adbc_ingest(identifier, upload, mode='create', db_schema_name=schema)
        except adbc.Error as error:
            # A table supplies fresh input. One-shot readers and unknown commit
            # outcomes cannot replay an upload safely.
            if not isinstance(data, pa.Table):
                raise
            if attempt == 19:
                raise
            validate_retry(error)
            time.sleep(random.uniform(.05, .1) * min(2 ** attempt, 16))


def upload_input(data):
    # A schema-only Flight upload has no messages. Recreate the zero-row
    # reader for each attempt from the retained immutable table.
    if not isinstance(data, pa.Table):
        return data
    if data.num_rows:
        return data
    batch = pa.RecordBatch.from_arrays(
        [pa.array([], type=f.type) for f in data.schema], schema=data.schema)
    return pa.RecordBatchReader.from_batches(data.schema, [batch])


def validate_retry(error):
    if error.status_code != adbc.AdbcStatusCode.IO:
        raise error
    if error.sqlstate == '40001':
        if 'snapshot catalog changed concurrently; retry the statement' in str(error):
            return
    if error.vendor_code == 14:
        validate_flight_retry(error)
        return
    raise error


def validate_flight_retry(error):
    if '[redis] snapshot catalog changed concurrently; retry the statement (40001)' not in str(error):
        raise error
    if '(Unavailable;' not in str(error):
        raise error


def run_model(connection, parameters):
    namespace = {'__name__': '__dbt_model__'}
    exec(compile(parameters['code'], 'dbt_model.py', 'exec'), namespace)
    session = Session(connection)
    result = arrow_result(namespace['model'](namespace['dbtObj'](lambda name: session.sql('select * from ' + name)), session))
    target = namespace['dbt_redis_adbc_target']
    return ingest(connection, target['identifier'], result, target['schema'])


class CsvSampler:
    def __init__(self, schema, parameters):
        self.stride = parameters['sample_every']
        self.limit = parameters['limit']
        self.row_name = parameters['row_number_column']
        self.schema = pa.schema([pa.field(self.row_name, pa.int64()), *schema]) if self.row_name else schema

    def select(self, batch, seen, emitted):
        positions = pa.array(range((-seen) % self.stride, batch.num_rows, self.stride), pa.int64())
        selected = batch.take(positions)
        if self.row_name:
            selected = pa.RecordBatch.from_arrays(
                [pc.add(positions, pa.scalar(seen + 1)), *selected.columns], schema=self.schema)
        if self.limit is not None:
            selected = selected.slice(0, max(0, self.limit - emitted))
        return selected

    def batches(self, reader):
        seen = emitted = 0
        for batch in reader:
            selected = self.select(batch, seen, emitted)
            seen += batch.num_rows
            emitted += selected.num_rows
            if selected.num_rows:
                yield selected
            if self.limit is not None and emitted >= self.limit:
                break


def load_csv(connection, parameters):
    types = parameters['column_types']
    with connection.cursor() as cursor:
        cursor.execute(parameters['probe_sql'])
        schema = cursor.fetch_arrow_table().schema
    reader = csv.open_csv(parameters['path'], convert_options=csv.ConvertOptions(
        column_types={field.name: field.type for field in schema}, include_columns=list(types),
        timestamp_parsers=[parameters['timestamp_format']], strings_can_be_null=True))
    sampler = CsvSampler(schema, parameters)

    # Validate the sampled input before replacing an existing relation.
    table = pa.Table.from_batches(sampler.batches(reader), schema=sampler.schema)
    with connection.cursor() as cursor:
        cursor.execute(parameters['create_schema_sql'])
        cursor.execute(parameters['drop_table_sql'])
    return ingest(connection, parameters['identifier'], table, parameters['schema'])


def main():
    request = json.load(sys.stdin)
    with contextlib.redirect_stdout(sys.stderr):
        with dbapi.connect(driver=request['driver'], entrypoint=request.get('entrypoint', 'AdbcDriverInit'), db_kwargs=request['options'], autocommit=True) as connection:
            for sql in request.get('init_sql', []):
                with connection.cursor() as cursor:
                    cursor.execute(sql)
                    cursor.fetchall()
            if request['operation'] == 'model':
                rows = run_model(connection, request['parameters'])
            elif request['operation'] == 'csv':
                rows = load_csv(connection, request['parameters'])
            else:
                raise ValueError('Unknown Arrow runtime operation')
    json.dump(rows, sys.stdout)


if __name__ == '__main__':
    main()
