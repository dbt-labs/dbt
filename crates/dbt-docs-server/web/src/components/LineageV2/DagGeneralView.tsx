import type { ReactNode } from 'react';
import {
  AlertTriangle,
  AlignLeft,
  Box,
  ChartColumn,
  Code,
  Columns3,
  Database,
  Hash,
  Layers,
  Lock,
  Package,
  ShieldCheck,
  Tag,
} from 'lucide-react';

import { paths } from '../../routes';
import { buildRelationName, DescriptionDisplay, RelationName } from '../../shared';
import type { Asset } from '../../shared/typings/domain/asset';
import { Badge } from '../ui/Badge';

/** How many tag pills render before collapsing the rest into a "+N" pill. */
const MAX_VISIBLE_TAGS = 3;

interface DetailRowProps {
  icon: ReactNode;
  label: string;
  children: ReactNode;
}

function DetailRow({ icon, label, children }: DetailRowProps) {
  return (
    <div className="flex items-start gap-3 py-3 first:pt-0 last:pb-0">
      <span className="mt-0.5 flex-none text-fgDecorative">{icon}</span>
      <span className="w-[100px] flex-none pt-px text-sm text-fgDecorative">
        {label}
      </span>
      <span className="min-w-0 flex-1 pt-px text-sm text-fgMain">{children}</span>
    </div>
  );
}

/** The root project (this docs site), not the resource's own package -- a
 *  project can bundle multiple installed packages, each with their own
 *  `packageName`, so these are two different rows (see PackageRow). Links
 *  out to the project overview, matching the mockups' underlined project name. */
function ProjectRow({ projectName }: { projectName: string }) {
  return (
    <DetailRow icon={<Package size={16} />} label="Project">
      <button
        type="button"
        onClick={() => window.location.assign(paths.home())}
        className="text-fgBrand hover:underline"
      >
        {projectName}
      </button>
    </DetailRow>
  );
}

function PackageRow({ packageName }: { packageName: string }) {
  return (
    <DetailRow icon={<Box size={16} />} label="Package">
      {packageName}
    </DetailRow>
  );
}

/** Renders nothing when there are no tags, so callers can include it
 *  unconditionally instead of repeating the length check per resource type. */
function TagsRow({ tags }: { tags: string[] | null | undefined }) {
  const list = tags ?? [];
  if (list.length === 0) return null;
  const visible = list.slice(0, MAX_VISIBLE_TAGS);
  const overflowCount = list.length - visible.length;
  return (
    <DetailRow icon={<Tag size={16} />} label="Tags">
      <span className="flex flex-wrap gap-1">
        {visible.map((tag) => (
          <Badge key={tag} text={tag} variant="secondary" />
        ))}
        {overflowCount > 0 && <Badge text={`+${overflowCount}`} variant="outline" />}
      </span>
    </DetailRow>
  );
}

/** DescriptionDisplay already renders a "This resource does not have a
 *  description" placeholder, so this row is always shown, never conditional. */
function DescriptionRow({ description }: { description: string | null }) {
  return (
    <DetailRow icon={<AlignLeft size={16} />} label="Description">
      <DescriptionDisplay description={description} className="text-sm" />
    </DetailRow>
  );
}

/**
 * Per-resource-type general-tab layouts, redesigned against the Figma
 * "Peek/Lineage" mockups (model) and current prod dbt Explorer panels (test,
 * metric, semantic model, seed, snapshot). Row order intentionally follows
 * each type's own reference exactly rather than forcing one shared order
 * across types -- that's how the prod panels actually look today.
 *
 * Deliberately omits every field that's dbt Cloud/Discovery-API-only (Health,
 * Con. queries, Popularity, Owner, the "Production" environment pill) -- none
 * of those exist in the OSS manifest data model, so there's nothing real to
 * show. The test mockup's Schema/Static Analysis/Store Failures(/As) and its
 * config JSON block are a different kind of gap: not Cloud-only, just not
 * currently selected by the detail query in `duckdb/details.ts` -- omitted
 * here too, but that one could be added later with a data-layer change.
 *
 * `saved_query` has no mockup reference yet; its layout below is extrapolated
 * from the same OSS-available fields as everything else (Project, Package,
 * Tags, Description).
 */
export function DagGeneralView({
  asset,
  projectName,
}: {
  asset: Asset;
  projectName: string;
}) {
  switch (asset.resourceType) {
    case 'model':
      return (
        <div className="divide-y divide-borderMuted">
          <ProjectRow projectName={projectName} />
          <PackageRow packageName={asset.packageName} />
          <DetailRow icon={<Columns3 size={16} />} label="Columns">
            <Badge text={String(asset.columns.length)} variant="secondary" />
          </DetailRow>
          {asset.language && (
            <DetailRow icon={<Code size={16} />} label="Language">
              {asset.language}
            </DetailRow>
          )}
          {asset.access && (
            <DetailRow icon={<Lock size={16} />} label="Access">
              {asset.access}
            </DetailRow>
          )}
          {asset.contractEnforced != null && (
            <DetailRow icon={<ShieldCheck size={16} />} label="Contract">
              {asset.contractEnforced ? 'True' : 'False'}
            </DetailRow>
          )}
          {asset.materializedType && (
            <DetailRow icon={<Layers size={16} />} label="Materialized">
              <Badge text={asset.materializedType} variant="outline" />
            </DetailRow>
          )}
          <TagsRow tags={asset.tags} />
          <DescriptionRow description={asset.description} />
        </div>
      );

    case 'test':
      return (
        <div className="divide-y divide-borderMuted">
          <DescriptionRow description={asset.description} />
          <ProjectRow projectName={projectName} />
          {asset.columnName && (
            <DetailRow icon={<Hash size={16} />} label="Column Name">
              {asset.columnName}
            </DetailRow>
          )}
          <PackageRow packageName={asset.packageName} />
          {asset.severity && (
            <DetailRow icon={<AlertTriangle size={16} />} label="Severity">
              {asset.severity.toUpperCase()}
            </DetailRow>
          )}
          <TagsRow tags={asset.tags} />
        </div>
      );

    case 'metric': {
      const { typeParams } = asset;
      return (
        <div className="divide-y divide-borderMuted">
          <DescriptionRow description={asset.description} />
          <ProjectRow projectName={projectName} />
          <DetailRow icon={<ChartColumn size={16} />} label="Type">
            {typeParams.kind}
          </DetailRow>
          <TagsRow tags={asset.tags} />
          {typeParams.kind === 'simple' && (
            <DetailRow icon={<Hash size={16} />} label="Expression">
              {typeParams.measure.name}
            </DetailRow>
          )}
          {typeParams.kind === 'cumulative' && typeParams.window && (
            <DetailRow icon={<Hash size={16} />} label="Window Granularity">
              {typeParams.window}
            </DetailRow>
          )}
          <PackageRow packageName={asset.packageName} />
        </div>
      );
    }

    case 'semantic_model':
      return (
        <div className="divide-y divide-borderMuted">
          <DescriptionRow description={asset.description} />
          <ProjectRow projectName={projectName} />
          <TagsRow tags={asset.tags} />
        </div>
      );

    case 'seed':
    case 'snapshot': {
      // `asset.relation` can be a non-null object with every field still null
      // (no warehouse catalog data resolved) -- check the buildable name, not
      // just object truthiness, or the row shows with an empty value.
      const relationName = asset.relation ? buildRelationName(asset.relation) : null;
      return (
        <div className="divide-y divide-borderMuted">
          <DescriptionRow description={asset.description} />
          <ProjectRow projectName={projectName} />
          {relationName && (
            <DetailRow icon={<Database size={16} />} label="Relation">
              <RelationName relation={asset.relation!} />
            </DetailRow>
          )}
          <TagsRow tags={asset.tags} />
          <PackageRow packageName={asset.packageName} />
        </div>
      );
    }

    case 'saved_query':
      return (
        <div className="divide-y divide-borderMuted">
          <DescriptionRow description={asset.description} />
          <ProjectRow projectName={projectName} />
          <PackageRow packageName={asset.packageName} />
          <TagsRow tags={asset.tags} />
        </div>
      );

    default:
      return null;
  }
}
