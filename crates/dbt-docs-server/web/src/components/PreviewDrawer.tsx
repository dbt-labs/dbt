import { useEffect, useRef } from 'react';
import { ExternalLink, X } from 'lucide-react';

import {
  RESOURCE_TYPE_LABEL,
  RESOURCE_TYPE_SINGULAR,
  ResourceTypeIcon,
} from '../lib/resourceType';
import type { Project } from '../shared';
import { type Asset, PageHeading } from '../shared';
import type { NodeSummary } from '../types';
import { Badge } from './ui/Badge';

interface Props {
  project: Project;
  previewId: string;
  /** What we already know from the loaded nodes list. Available immediately. */
  summary: NodeSummary | null;
  /** Full detail, fetched lazily. Null while loading. */
  detail: Asset | null;
  onClose(): void;
  onOpenFull(uniqueId: string): void;
}

/** Right-side peek drawer. Third column ≥1280px, modal overlay below.
 *  Detail data is fetched + cached in App.tsx; we render summary fields
 *  immediately while detail streams in. */
export function PreviewDrawer({
  project,
  previewId,
  summary,
  detail,
  onClose,
  onOpenFull,
}: Props) {
  const titleRef = useRef<HTMLButtonElement>(null);

  // Move focus to the title link when the drawer opens.
  useEffect(() => {
    titleRef.current?.focus();
  }, [previewId]);

  const relation = detail && 'relation' in detail ? detail.relation : null;
  const resourceType = summary?.resource_type ?? detail?.resourceType ?? 'model';
  const name = summary?.name ?? detail?.name ?? previewId;
  const description = detail?.description ?? summary?.description ?? null;
  const materialized = detail?.materialized ?? summary?.materialized ?? null;
  const schema = relation?.schema ?? summary?.schema_name ?? null;
  const database = relation?.database ?? summary?.database_name ?? null;
  const pkg = detail?.packageName ?? summary?.package_name ?? null;
  const dependsOn = detail?.dependsOn?.length;
  const referencedBy = detail?.referencedBy?.length;
  const singular = RESOURCE_TYPE_SINGULAR[resourceType] ?? resourceType;
  const plural = RESOURCE_TYPE_LABEL[resourceType] ?? `${singular}s`;
  const isLoading = !detail;

  return (
    <>
      <button
        type="button"
        className="preview-drawer__backdrop"
        aria-label="Close preview"
        onClick={onClose}
      />
      <aside
        className="preview-drawer"
        role="dialog"
        aria-labelledby="preview-drawer-title"
      >
        {/* Top bar: breadcrumb + close */}
        <header className="preview-drawer__head">
          <div className="preview-drawer__crumb">
            <span>{project.name}</span>
            <span aria-hidden>/</span>
            <span>{plural}</span>
          </div>
          <div className="preview-drawer__head-actions">
            <button
              type="button"
              className="preview-drawer__action"
              onClick={onClose}
              aria-label="Close preview"
              title="Close"
            >
              <X className="size-3" />
            </button>
          </div>
        </header>

        {/* Title row: type icon + name (link) + open-external icon + Badge */}
        <PageHeading
          className="preview-drawer__title-row"
          additional={{
            left: <ResourceTypeIcon type={resourceType} className="size-3.5" />,
            right: <Badge text={singular} variant="secondary" size="xs" />,
          }}
        >
          <button
            ref={titleRef}
            type="button"
            id="preview-drawer-title"
            className="preview-drawer__title-link"
            onClick={() => onOpenFull(previewId)}
            title="Open full asset page"
          >
            {name}
            <span className="preview-drawer__title-open" aria-hidden>
              <ExternalLink className="size-3" />
            </span>
          </button>
        </PageHeading>

        {description && <p className="preview-drawer__desc">{description}</p>}

        {/* Phase 1 — honest grid of fields we actually have. */}
        <section className="preview-drawer__grid" aria-label="Asset details">
          <Cell
            label="Materialization"
            value={materialized}
            loading={isLoading && materialized == null}
          />
          <Cell label="Schema" value={schema} loading={isLoading && schema == null} />
          <Cell
            label="Database"
            value={database}
            loading={isLoading && database == null}
          />
          <Cell label="Package" value={pkg} loading={isLoading && pkg == null} />
          <Cell
            label="Depends on"
            value={dependsOn != null ? dependsOn.toLocaleString() : null}
            loading={isLoading}
          />
          <Cell
            label="Downstream"
            value={referencedBy != null ? referencedBy.toLocaleString() : null}
            loading={isLoading}
          />
        </section>

        {/* Phase 2+ — fields not yet exposed. */}
        <section className="preview-drawer__pending" aria-label="Coming soon">
          <h4>Coming soon</h4>
          <ul>
            <li>
              <span className="preview-drawer__pending-label">Last run / test</span>
              <span className="preview-drawer__pending-hint">
                When run results are exported alongside the nodes
              </span>
            </li>
            <li>
              <span className="preview-drawer__pending-label">Tags · Owner</span>
              <span className="preview-drawer__pending-hint">
                When schema.yml <code>meta</code> surfaces in the parquet
              </span>
            </li>
          </ul>
        </section>
      </aside>
    </>
  );
}

function Cell({
  label,
  value,
  loading,
}: {
  label: string;
  value: string | null;
  loading?: boolean;
}) {
  return (
    <div className="preview-drawer__cell">
      <div className="preview-drawer__cell-label">{label}</div>
      <div className="preview-drawer__cell-value">
        {loading ? (
          <span className="preview-drawer__skeleton" aria-hidden />
        ) : (
          (value ?? '')
        )}
      </div>
    </div>
  );
}
