import { ColumnTable, Entry } from './ColumnTable';

export type AssetColumnItem = {
  name: string;
  description?: string | null;
  dataType?: string | null;
};

export type AssetColumnsProps = {
  columns: AssetColumnItem[];
  isLoading?: boolean;
};

export function AssetColumns({ columns, isLoading }: AssetColumnsProps) {
  const entries: Entry[] = columns.map((col: AssetColumnItem) => ({
    key: col.name,
    data: col.dataType
      ? `${col.description ?? '—'} (${col.dataType})`
      : (col.description ?? '—'),
  }));
  return <ColumnTable tableEntries={entries} isLoading={isLoading} />;
}
