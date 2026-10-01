import { useEffect, useMemo, useRef, useState } from 'react';

import type { AssetFilters } from '../App';
import {
  inferModelingLayer,
  RESOURCE_TYPE_LABEL,
  ResourceBadge,
  ResourceTypeIcon,
} from '../lib/resourceType';
import type { Project } from '../shared';
import { SimpleLinkBreadcrumbs, useResourceLink } from '../shared';
import type { NodeSummary } from '../types';

const PAGE_SIZE = 200;

type SortKey = 'az' | 'za';

interface Props {
  project: Project;
  nodes: NodeSummary[];
  query: string;
  filters: AssetFilters;
  previewId: string | null;
  onPeek(uniqueId: string): void;
}

const COLUMN_KEYS = [
  'name',
  'resource_type',
  'package',
  'modeling_layer',
  'materialization',
  'schema',
  'description',
] as const;
type ColumnKey = (typeof COLUMN_KEYS)[number];

const DEFAULT_WIDTHS: Record<ColumnKey, number> = {
  name: 28,
  resource_type: 12,
  package: 16,
  modeling_layer: 12,
  materialization: 12,
  schema: 12,
  description: 28,
};

export function AssetListView({
  project,
  nodes,
  query,
  filters,
  previewId,
  onPeek,
}: Props) {
  const [shown, setShown] = useState<number>(PAGE_SIZE);
  const [sort, setSort] = useState<SortKey>('az');
  const [widths, setWidths] = useState<Record<ColumnKey, number>>(DEFAULT_WIDTHS);
  const links = useResourceLink();

  // The "single type" projection of the multi-select resourceType filter.
  // Used for breadcrumb, title, and the hide-resource-type-column heuristic
  // (no point showing it when every row is the same type).
  const singleType = filters.resourceType.length === 1 ? filters.resourceType[0] : null;

  // Filter pipeline.
  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const rt = filters.resourceType;
    const ml = filters.modelingLayer;
    const mt = filters.materialization;
    const pk = filters.pkg;
    return nodes.filter((n) => {
      if (rt.length > 0 && !rt.includes(n.resource_type)) return false;
      if (ml.length > 0) {
        const layer = inferModelingLayer(n.original_file_path);
        if (!layer || !ml.includes(layer)) return false;
      }
      if (mt.length > 0 && (!n.materialized || !mt.includes(n.materialized)))
        return false;
      if (pk.length > 0 && (!n.package_name || !pk.includes(n.package_name)))
        return false;
      if (
        needle &&
        !n.name.toLowerCase().includes(needle) &&
        !n.unique_id.toLowerCase().includes(needle)
      ) {
        return false;
      }
      return true;
    });
  }, [nodes, filters, query]);

  const sorted = useMemo(() => {
    const arr = [...filtered];
    if (sort === 'az') {
      arr.sort((a, b) => a.name.localeCompare(b.name));
    } else {
      arr.sort((a, b) => b.name.localeCompare(a.name));
    }
    return arr;
  }, [filtered, sort]);

  const typeTotal = useMemo(() => {
    const rt = filters.resourceType;
    if (rt.length === 0) return nodes.length;
    return nodes.filter((n) => rt.includes(n.resource_type)).length;
  }, [nodes, filters.resourceType]);

  const visible = sorted.slice(0, shown);
  const hasMore = shown < sorted.length;
  const title = singleType
    ? (RESOURCE_TYPE_LABEL[singleType] ?? singleType)
    : filters.resourceType.length > 1
      ? `${filters.resourceType.length} resource types`
      : 'All assets';
  const visibleCols = COLUMN_KEYS.filter((k) => k !== 'resource_type' || !singleType);

  return (
    <div className="asset-list">
      <SimpleLinkBreadcrumbs
        className="font-caption mb-3 block text-fgDecorative"
        breadcrumbs={[{ text: project.name, href: links.home() }, { text: title }]}
      />

      <header className="asset-list__header">
        <div className="asset-list__title-row">
          {singleType && <ResourceTypeIcon type={singleType} className="size-4" />}
          <h1 className="asset-list__title">{title}</h1>
        </div>
        <div className="asset-list__count">
          {sorted.length.toLocaleString()} of {typeTotal.toLocaleString()}
        </div>
      </header>

      <p className="asset-list__filter-caption">
        Use the filter pane to narrow results.
      </p>

      <div className="asset-list__toolbar">
        <div className="asset-list__sort" role="radiogroup" aria-label="Sort assets">
          <button
            type="button"
            role="radio"
            aria-checked={sort === 'az'}
            className={`asset-list__sort-btn ${sort === 'az' ? 'is-active' : ''}`}
            onClick={() => setSort('az')}
          >
            A→Z
          </button>
          <button
            type="button"
            role="radio"
            aria-checked={sort === 'za'}
            className={`asset-list__sort-btn ${sort === 'za' ? 'is-active' : ''}`}
            onClick={() => setSort('za')}
          >
            Z→A
          </button>
        </div>
      </div>

      <ActiveFilters filters={filters} />

      {sorted.length === 0 ? (
        <p className="asset-list__empty">
          {query.trim()
            ? `No matches for "${query.trim()}" with the current filters.`
            : 'No assets match the current filters. Clear filters in the side panel to widen results.'}
        </p>
      ) : (
        <table className="asset-list__table" style={tableLayout(visibleCols, widths)}>
          <colgroup>
            {visibleCols.map((c) => (
              <col key={c} style={{ width: `${widths[c]}%` }} />
            ))}
          </colgroup>
          <thead>
            <tr>
              {visibleCols.map((c, idx) => (
                <Th
                  key={c}
                  label={COLUMN_LABEL[c]}
                  isLast={idx === visibleCols.length - 1}
                  onResize={(delta) => resizeColumn(setWidths, visibleCols, c, delta)}
                />
              ))}
            </tr>
          </thead>
          <tbody>
            {visible.map((n) => (
              <Row
                key={n.unique_id}
                node={n}
                columns={visibleCols}
                projectName={project.name}
                active={previewId === n.unique_id}
                onActivate={onPeek}
              />
            ))}
          </tbody>
        </table>
      )}

      <footer className="asset-list__footer">
        {hasMore ? (
          <button
            type="button"
            className="asset-list__load-more"
            onClick={() => setShown((s) => s + PAGE_SIZE)}
          >
            Load {Math.min(PAGE_SIZE, sorted.length - shown)} more…
          </button>
        ) : null}
        <div className="asset-list__progress">
          Showing {visible.length.toLocaleString()} of {sorted.length.toLocaleString()}
        </div>
      </footer>
    </div>
  );
}

const COLUMN_LABEL: Record<ColumnKey, string> = {
  name: 'Name',
  resource_type: 'Resource type',
  package: 'Package',
  modeling_layer: 'Modeling layer',
  materialization: 'Materialization',
  schema: 'Schema',
  description: 'Description',
};

function tableLayout(_cols: readonly ColumnKey[], _w: Record<ColumnKey, number>) {
  return { tableLayout: 'fixed' as const };
}

/** Drag a column's right edge: redistribute the delta into the next visible
 *  column so the total width is conserved. Minimum 6% per column. */
function resizeColumn(
  setWidths: (
    updater: (prev: Record<ColumnKey, number>) => Record<ColumnKey, number>,
  ) => void,
  visible: readonly ColumnKey[],
  col: ColumnKey,
  deltaPct: number,
) {
  const idx = visible.indexOf(col);
  const next = visible[idx + 1];
  if (!next) return;
  const MIN = 6;
  setWidths((prev) => {
    const a = prev[col] + deltaPct;
    const b = prev[next] - deltaPct;
    if (a < MIN || b < MIN) return prev;
    return { ...prev, [col]: a, [next]: b };
  });
}

/* ---------- Active filter summary ---------- */

function ActiveFilters({ filters }: { filters: AssetFilters }) {
  const pills: { label: string; values: string[] }[] = [];
  if (filters.modelingLayer.length > 0)
    pills.push({ label: 'Modeling layer', values: filters.modelingLayer });
  if (filters.materialization.length > 0)
    pills.push({ label: 'Materialization', values: filters.materialization });
  if (filters.pkg.length > 0) pills.push({ label: 'Package', values: filters.pkg });

  if (pills.length === 0) return null;

  return (
    <div className="asset-list__active-filters">
      <span className="asset-list__active-filters-label">Filters</span>
      {pills.map((p) => (
        <span key={p.label} className="asset-list__active-filter">
          <span className="asset-list__active-filter-key">{p.label}:</span>
          <span className="asset-list__active-filter-val">{p.values.join(', ')}</span>
        </span>
      ))}
    </div>
  );
}

/* ---------- TH with resize handle ---------- */

function Th({
  label,
  isLast,
  onResize,
}: {
  label: string;
  isLast: boolean;
  onResize: (deltaPct: number) => void;
}) {
  const startX = useRef(0);
  const tableWidth = useRef(0);

  const onPointerDown = (e: React.PointerEvent<HTMLSpanElement>) => {
    e.preventDefault();
    const table = e.currentTarget.closest('table');
    if (!table) return;
    tableWidth.current = table.getBoundingClientRect().width;
    startX.current = e.clientX;

    const onMove = (ev: PointerEvent) => {
      const deltaPx = ev.clientX - startX.current;
      const deltaPct = (deltaPx / tableWidth.current) * 100;
      onResize(deltaPct);
      startX.current = ev.clientX;
    };
    const onUp = () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  };

  return (
    <th>
      <span className="asset-list__th-label">{label}</span>
      {!isLast && (
        <span
          className="asset-list__th-resize"
          role="separator"
          aria-orientation="vertical"
          onPointerDown={onPointerDown}
        />
      )}
    </th>
  );
}

/* ---------- Row ---------- */

function Row({
  node,
  columns,
  projectName,
  active,
  onActivate,
}: {
  node: NodeSummary;
  columns: readonly ColumnKey[];
  projectName: string;
  active: boolean;
  onActivate: (id: string) => void;
}) {
  const layer = inferModelingLayer(node.original_file_path);
  const isUserProject = node.package_name === projectName;
  return (
    <tr
      className={`asset-list__row ${active ? 'is-active' : ''}`}
      tabIndex={0}
      onClick={() => onActivate(node.unique_id)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onActivate(node.unique_id);
        }
      }}
    >
      {columns.map((c) => {
        switch (c) {
          case 'name':
            return (
              <td key={c} className="asset-list__col-name">
                <div className="asset-list__name-cell">
                  <ResourceTypeIcon type={node.resource_type} className="size-3" />
                  <span>{node.name}</span>
                </div>
              </td>
            );
          case 'resource_type':
            return (
              <td key={c}>
                <ResourceBadge type={node.resource_type} size="xs" />
              </td>
            );
          case 'package':
            return (
              <td key={c} className={isUserProject ? 'asset-list__pkg-self' : ''}>
                {node.package_name ?? ''}
              </td>
            );
          case 'modeling_layer':
            return <td key={c}>{layer ?? ''}</td>;
          case 'materialization':
            return <td key={c}>{node.materialized ?? ''}</td>;
          case 'schema':
            return (
              <td key={c} className="asset-list__mono">
                {node.schema_name ?? ''}
              </td>
            );
          case 'description':
            return (
              <td key={c} className="asset-list__desc">
                {node.description?.trim() || ''}
              </td>
            );
          default:
            return null;
        }
      })}
    </tr>
  );
}

// Suppress unused-import warning when bundlers tree-shake aggressively.
void useEffect;
