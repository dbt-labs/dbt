import psycopg2

from contextlib import contextmanager

import dbt.adapters.default
import dbt.compat
import dbt.exceptions
import agate

from dbt.logger import GLOBAL_LOGGER as logger


class PostgresAdapter(dbt.adapters.default.DefaultAdapter):

    @classmethod
    @contextmanager
    def exception_handler(cls, profile, sql, model_name=None,
                          connection_name=None):
        try:
            yield

        except psycopg2.DatabaseError as e:
            logger.debug('Postgres error: {}'.format(str(e)))

            try:
                # attempt to release the connection
                cls.release_connection(profile, connection_name)
            except psycopg2.Error:
                logger.debug("Failed to release connection!")
                pass

            raise dbt.exceptions.DatabaseException(
                dbt.compat.to_string(e).strip())

        except Exception as e:
            logger.debug("Error running SQL: %s", sql)
            logger.debug("Rolling back transaction.")
            cls.release_connection(profile, connection_name)
            raise dbt.exceptions.RuntimeException(e)

    @classmethod
    def type(cls):
        return 'postgres'

    @classmethod
    def date_function(cls):
        return 'datenow()'

    @classmethod
    def get_status(cls, cursor):
        return cursor.statusmessage

    @classmethod
    def open_connection(cls, connection):
        if connection.get('state') == 'open':
            logger.debug('Connection is already open, skipping open.')
            return connection

        result = connection.copy()

        try:
            credentials = connection.get('credentials', {})
            handle = psycopg2.connect(
                dbname=credentials.get('dbname'),
                user=credentials.get('user'),
                host=credentials.get('host'),
                password=credentials.get('pass'),
                port=credentials.get('port'),
                connect_timeout=10)

            result['handle'] = handle
            result['state'] = 'open'
        except psycopg2.Error as e:
            logger.debug("Got an error when attempting to open a postgres "
                         "connection: '{}'"
                         .format(e))

            result['handle'] = None
            result['state'] = 'fail'

            raise dbt.exceptions.FailedToConnectException(str(e))

        return result

    @classmethod
    def cancel_connection(cls, profile, connection):
        connection_name = connection.get('name')
        pid = connection.get('handle').get_backend_pid()

        sql = "select pg_terminate_backend({})".format(pid)

        logger.debug("Cancelling query '{}' ({})".format(connection_name, pid))

        _, cursor = cls.add_query(profile, sql, 'master')
        res = cursor.fetchone()

        logger.debug("Cancel query '{}': {}".format(connection_name, res))

    # DATABASE INSPECTION FUNCTIONS
    # These require the profile AND project, as they need to know
    # database-specific configs at the project level.
    @classmethod
    def alter_column_type(cls, profile, project, schema, table, column_name,
                          new_column_type, model_name=None):
        """
        1. Create a new column (w/ temp name and correct type)
        2. Copy data over to it
        3. Drop the existing column (cascade!)
        4. Rename the new column to existing column
        """

        relation = cls.Relation.create(schema=schema, identifier=table)

        opts = {
            "relation": relation,
            "old_column": column_name,
            "tmp_column": "{}__dbt_alter".format(column_name),
            "dtype": new_column_type
        }

        sql = """
        alter table {relation} add column "{tmp_column}" {dtype};
        update {relation} set "{tmp_column}" = "{old_column}";
        alter table {relation} drop column "{old_column}" cascade;
        alter table {relation} rename column "{tmp_column}" to "{old_column}";
        """.format(**opts).strip()  # noqa

        connection, cursor = cls.add_query(profile, sql, model_name)

        return connection, cursor

    @classmethod
    def list_relations(cls, profile, project, schema, model_name=None):
        sql = """
        select tablename as name, schemaname as schema, 'table' as type from pg_tables
        where schemaname ilike '{schema}'
        union all
        select viewname as name, schemaname as schema, 'view' as type from pg_views
        where schemaname ilike '{schema}'
        """.format(schema=schema).strip()  # noqa

        connection, cursor = cls.add_query(profile, sql, model_name,
                                           auto_begin=False)

        results = cursor.fetchall()

        return [cls.Relation.create(
            database=profile.get('dbname'),
            schema=_schema,
            identifier=name,
            quote_policy={
                'schema': True,
                'identifier': True
            },
            type=type)
                for (name, _schema, type) in results]

    @classmethod
    def get_existing_schemas(cls, profile, project, model_name=None):
        sql = "select distinct nspname from pg_namespace"

        connection, cursor = cls.add_query(profile, sql, model_name,
                                           auto_begin=False)
        results = cursor.fetchall()

        return [row[0] for row in results]

    @classmethod
    def check_schema_exists(cls, profile, project, schema, model_name=None):
        sql = """
        select count(*) from pg_namespace where nspname = '{schema}'
        """.format(schema=schema).strip()  # noqa

        connection, cursor = cls.add_query(profile, sql, model_name,
                                           auto_begin=False)
        results = cursor.fetchone()

        return results[0] > 0

    @classmethod
    def get_catalog_for_schemas(cls, profile, schemas=None):
        # TODO: we want to move this into separate files that users can modify
        # so they can generate their own.
        sql = """
            with tables as (
            select
                table_schema,
                table_name,
                table_type

            from information_schema.tables

            ),

            columns as (

                select
                    table_schema,
                    table_name,
                    null as table_comment,

                    column_name,
                    ordinal_position as column_index,
                    data_type as column_type,
                    null as column_comment


                from information_schema.columns

            )

            select *
            from tables
            join columns using (table_schema, table_name)

            where table_schema != 'information_schema'
              and table_schema not like 'pg_%'
        """.strip()
        # TODO: make sure I can just ignore status.
        _, results = cls.execute(profile, sql, fetch=True)
        # ok, have an agate.Table now consisting of my results. Filter out
        # schemas if necessary (this should probably happen in SQL, right?).
        if schemas is not None:
            results = results.where(lambda r: r['table_schema'] in schemas)
        return [dict(zip(results.column_names, row)) for row in results]

    @classmethod
    def convert_text_type(cls, agate_table, col_idx):
        return "text"

    @classmethod
    def convert_number_type(cls, agate_table, col_idx):
        decimals = agate_table.aggregate(agate.MaxPrecision(col_idx))
        return "float8" if decimals else "integer"

    @classmethod
    def convert_boolean_type(cls, agate_table, col_idx):
        return "boolean"

    @classmethod
    def convert_datetime_type(cls, agate_table, col_idx):
        return "timestamp without time zone"

    @classmethod
    def convert_date_type(cls, agate_table, col_idx):
        return "date"

    @classmethod
    def convert_time_type(cls, agate_table, col_idx):
        return "time"
