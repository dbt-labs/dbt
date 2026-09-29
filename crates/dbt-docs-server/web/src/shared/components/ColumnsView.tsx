import { FC, ReactNode, useMemo, useState } from 'react';
import { Search } from 'lucide-react';

import { Input } from '../../components/ui/Input';
import { ColumnCardShell } from './ColumnCardShell';

export type ColumnItem = {
  name: string;
  type?: string | null;
  description?: string | null;
  isPrimaryKey?: boolean;
  constraints?: Array<{ name?: string | null; type?: string }>;
};

export type ColumnsViewProps = {
  columns: ColumnItem[];
  isLoading?: boolean;
  disableSearch?: boolean;
  renderItem?: (col: ColumnItem, query: string) => ReactNode;
  /** When true each default row renders an expand caret; clicking it
   *  reveals `renderExpanded(col)`. */
  expandable?: boolean;
  renderExpanded?: (col: ColumnItem) => ReactNode;
  emptyState?: ReactNode;
  className?: string;
};

const DefaultColumnRow: FC<{
  col: ColumnItem;
  expandable?: boolean;
  renderExpanded?: (col: ColumnItem) => ReactNode;
}> = ({ col, expandable, renderExpanded }) => {
  const [expanded, setExpanded] = useState(false);
  return (
    <ColumnCardShell
      name={col.name}
      type={col.type}
      isPrimaryKey={col.isPrimaryKey}
      constraints={col.constraints}
      expandable={expandable}
      expanded={expanded}
      onToggleExpanded={() => setExpanded((v) => !v)}
      toggleTooltip={{ open: 'Hide column lineage', closed: 'Show column lineage' }}
      description={
        col.description ? (
          <p className="mt-1 text-sm text-fgDecorative">{col.description}</p>
        ) : null
      }
      expandedBody={expandable && expanded ? renderExpanded?.(col) : null}
    />
  );
};

export const ColumnsView: FC<ColumnsViewProps> = ({
  columns,
  isLoading,
  disableSearch,
  renderItem,
  expandable,
  renderExpanded,
  emptyState,
  className = '',
}) => {
  const [query, setQuery] = useState('');

  const filtered = useMemo(() => {
    if (disableSearch || !query.trim()) return columns;
    const q = query.toLowerCase();
    return columns.filter(
      (c) =>
        c.name.toLowerCase().includes(q) ||
        (c.type ?? '').toLowerCase().includes(q) ||
        (c.description ?? '').toLowerCase().includes(q),
    );
  }, [columns, query, disableSearch]);

  return (
    <div className={className}>
      {!disableSearch && (
        <Input
          testId="columnSearch"
          name="Columns"
          className="my-4 mb-4 w-64"
          isEdit
          placeholder="Search for columns"
          startIcon={{ icon: <Search className="size-3" /> }}
          onChange={(e) => setQuery(e.target.value)}
        />
      )}
      {isLoading ? (
        <div className="text-fgDecorative">Loading...</div>
      ) : filtered.length === 0 ? (
        (emptyState ?? <div className="text-fgDecorative">No columns found.</div>)
      ) : (
        <ul aria-label="Columns" className="space-y-2">
          {filtered.map((col) =>
            renderItem ? (
              renderItem(col, query)
            ) : (
              <DefaultColumnRow
                key={col.name}
                col={col}
                expandable={expandable}
                renderExpanded={renderExpanded}
              />
            ),
          )}
        </ul>
      )}
    </div>
  );
};
