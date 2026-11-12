/** Browser data layer, independent of application components and styles. */
export {
  createEngine,
  type DuckDbEngine,
  type EngineOptions,
  isParquetBytes,
  type TableName,
} from './data-sources/duckdb/engine';
