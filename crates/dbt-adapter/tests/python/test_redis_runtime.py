"""Local Arrow runtime tests; no dbt Python package or Redis server required."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import MagicMock, patch

import pyarrow as pa
import pyarrow.csv as csv

path = Path(__file__).resolve().parents[2] / 'src/python/redis/runtime.py'
spec = importlib.util.spec_from_file_location('redis_runtime', path)
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class RuntimeTests(unittest.TestCase):
    def test_arrow_results_and_invalid_result(self):
        batch = pa.record_batch({'id': [1, 2]})
        self.assertEqual(runtime.arrow_result(batch).num_rows, 2)
        reader = pa.RecordBatchReader.from_batches(batch.schema, [batch])
        self.assertIs(runtime.arrow_result(reader), reader)
        with self.assertRaises(TypeError):
            runtime.arrow_result(None)

    def load_csv(self, text, **overrides):
        connection = MagicMock()
        cursor = connection.cursor.return_value.__enter__.return_value
        schema = pa.schema([('id', pa.int64()), ('name', pa.string())])
        cursor.fetch_arrow_table.return_value = pa.Table.from_batches([], schema=schema)
        cursor.adbc_ingest.side_effect = lambda name, data, **kw: data.num_rows if isinstance(data, pa.Table) else data.read_all().num_rows
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'data.csv'
            path.write_text(text)
            parameters = dict(schema='test', identifier='sample', path=str(path),
                              column_types={'id': 'BIGINT', 'name': 'VARCHAR'}, probe_sql='select cast(null as bigint) as id, cast(null as varchar) as name where false', create_schema_sql='create schema test', drop_table_sql='drop table test.sample', limit=None,
                              sample_every=1, row_number_column='file_row', timestamp_format='%Y-%m-%d %H:%M:%S')
            parameters.update(overrides)
            original_open = csv.open_csv
            def small_batches(*args, **kwargs):
                return original_open(*args, read_options=csv.ReadOptions(block_size=32), **kwargs)
            with patch.object(runtime.csv, 'open_csv', side_effect=small_batches):
                rows = runtime.load_csv(connection, parameters)
        return rows, cursor

    def test_sampling_and_row_numbers_cross_batch_boundaries(self):
        text = 'id,name\n' + ''.join(f'{index},value{index}\n' for index in range(1, 12))
        rows, cursor = self.load_csv(text, sample_every=3, limit=3)
        table = cursor.adbc_ingest.call_args.args[1]
        self.assertEqual(rows, 3)
        self.assertEqual(table['file_row'].to_pylist(), [1, 4, 7])
        self.assertEqual(table['id'].to_pylist(), [1, 4, 7])

    def test_empty_csv_preserves_typed_schema(self):
        rows, cursor = self.load_csv('id,name\n')
        table = cursor.adbc_ingest.call_args.args[1]
        self.assertEqual(rows, 0)
        self.assertIsInstance(table, pa.RecordBatchReader)
        self.assertEqual(table.schema.names, ['file_row', 'id', 'name'])
        self.assertEqual(table.schema.field('id').type, pa.int64())

    @patch.object(runtime.time, 'sleep')
    def test_table_ingest_retries_only_refused_publications(self, sleep):
        connection = MagicMock()
        cursor = connection.cursor.return_value.__enter__.return_value
        error = runtime.adbc.OperationalError(
            'snapshot catalog changed concurrently; retry the statement',
            status_code=runtime.adbc.AdbcStatusCode.IO, sqlstate='40001')
        cursor.adbc_ingest.side_effect = [error, 2]
        self.assertEqual(runtime.ingest(connection, 't', pa.table({'id': [1, 2]}), 's'), 2)
        self.assertEqual(cursor.adbc_ingest.call_count, 2)
        sleep.assert_called_once()

    @patch.object(runtime.time, 'sleep')
    def test_empty_flight_retry_uses_fresh_reader_after_first_was_consumed(self, sleep):
        connection = MagicMock()
        cursor = connection.cursor.return_value.__enter__.return_value
        readers = []
        schema = pa.schema([('id', pa.int64())])
        def upload(name, reader, **kwargs):
            readers.append(reader)
            batches = list(reader)
            self.assertEqual(len(batches), 1)
            self.assertEqual(batches[0].num_rows, 0)
            self.assertEqual(reader.schema, schema)
            if len(readers) == 1:
                raise runtime.adbc.OperationalError(
                    '[redis] snapshot catalog changed concurrently; retry the statement (40001) (Unavailable; FlightSQL.BulkIngest.Execute)',
                    status_code=runtime.adbc.AdbcStatusCode.IO, vendor_code=14)
            return 0
        cursor.adbc_ingest.side_effect = upload
        self.assertEqual(runtime.ingest(connection, 't', pa.Table.from_batches([], schema=schema), 's'), 0)
        self.assertIsNot(readers[0], readers[1])
        sleep.assert_called_once()

    @patch.object(runtime.time, 'sleep')
    def test_reader_and_unknown_outcomes_are_not_replayed(self, sleep):
        for data, message in [
            (pa.table({'id': [1]}).to_reader(), 'snapshot catalog changed concurrently; retry the statement'),
            (pa.table({'id': [1]}), 'publication outcome unknown'),
        ]:
            connection = MagicMock()
            cursor = connection.cursor.return_value.__enter__.return_value
            cursor.adbc_ingest.side_effect = runtime.adbc.OperationalError(
                message, status_code=runtime.adbc.AdbcStatusCode.IO, sqlstate='40001')
            with self.assertRaises(runtime.adbc.Error):
                runtime.ingest(connection, 't', data, 's')
            cursor.adbc_ingest.assert_called_once()
        sleep.assert_not_called()

    @patch.object(runtime.time, 'sleep')
    def test_flight_retry_is_bounded_and_does_not_replay_one_shot_input(self, sleep):
        message = '[redis] snapshot catalog changed concurrently; retry the statement (40001) (Unavailable; FlightSQL.BulkIngest.Execute)'
        for data, expected_calls in [(pa.table({'id': [1]}), 20), (pa.table({'id': [1]}).to_reader(), 1)]:
            connection = MagicMock()
            cursor = connection.cursor.return_value.__enter__.return_value
            cursor.adbc_ingest.side_effect = runtime.adbc.OperationalError(
                message, status_code=runtime.adbc.AdbcStatusCode.IO, vendor_code=14)
            with self.assertRaises(runtime.adbc.Error):
                runtime.ingest(connection, 't', data, 's')
            self.assertEqual(cursor.adbc_ingest.call_count, expected_calls)
        self.assertEqual(sleep.call_count, 19)

    def test_bad_csv_does_not_replace_existing_table(self):
        connection = MagicMock()
        cursor = connection.cursor.return_value.__enter__.return_value
        cursor.fetch_arrow_table.return_value = pa.table({'id': pa.array([], pa.int64())})
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'bad.csv'
            path.write_text('id\nnot-a-number\n')
            with self.assertRaises(pa.ArrowInvalid):
                runtime.load_csv(connection, dict(schema='test', identifier='sample', path=str(path),
                    column_types={'id': 'BIGINT'}, probe_sql='select cast(null as bigint) as id where false', create_schema_sql='create schema test', drop_table_sql='drop table test.sample', limit=None, sample_every=1,
                    row_number_column=None, timestamp_format='%Y-%m-%d %H:%M:%S'))
        self.assertFalse(any('drop table' in call.args[0] for call in cursor.execute.call_args_list))
        cursor.adbc_ingest.assert_not_called()


if __name__ == '__main__':
    unittest.main()
