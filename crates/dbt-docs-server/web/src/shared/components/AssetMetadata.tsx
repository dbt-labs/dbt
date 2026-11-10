import { ColumnTable, Entry } from './ColumnTable';
import { buildRelationName, RelationName } from './RelationName';
import type { DecorateOutboundHref, UserState } from './upgrade/types';
import { UpgradeRow } from './upgrade/UpgradeRow';

export type KnownMetadataResourceType =
  'model' | 'seed' | 'snapshot' | 'source' | 'exposure' | 'macro' | 'saved_query';

export type AssetMetadataProps = {
  resourceType: KnownMetadataResourceType | (string & {});
  uniqueId: string;
  packageName: string | null;
  relation: {
    database: string | null;
    schema: string | null;
    identifier: string | null;
  } | null;
  relationName?: string | null;
  loader?: string | null;
  language?: string | null;
  accessLevel?: string | null;
  contractEnforced?: boolean | null;
  group?: string | null;
  materialization?: string | null;
  tags?: string[] | null;
  meta?: Record<string, unknown> | null;
  filePath?: string | null;
  exposureType?: string | null;
  maturity?: string | null;
  url?: string | null;
  ownerName?: string | null;
  ownerEmail?: string | null;
  isLoading?: boolean;
  /** Drives the in-table query-history upsell row. Null suppresses the row
   *  (e.g. while capabilities are still loading). */
  userState?: UserState | null;
  /** Decorates the query-history upsell CTA href at click time (e.g. referral
   *  UTM params). Defaults to identity, so consumers that omit it see no
   *  change. */
  decorateOutboundHref?: DecorateOutboundHref;
  compact?: boolean;
};

// backward-compat alias
export type ModelMetadataProps = AssetMetadataProps;

function relationEntry(props: AssetMetadataProps): Entry | null {
  if (props.relation && buildRelationName(props.relation)) {
    return {
      key: 'Relation',
      data: <RelationName relation={props.relation} />,
      disableTooltip: true,
    };
  }
  if (props.relationName) {
    return { key: 'Relation', data: props.relationName };
  }
  return null;
}

function modelToEntries(props: AssetMetadataProps): Entry[] {
  const entries: Entry[] = [];
  if (props.packageName) entries.push({ key: 'Package', data: props.packageName });
  const rel = relationEntry(props);
  if (rel) entries.push(rel);
  if (props.language) entries.push({ key: 'Language', data: props.language });
  if (props.accessLevel) entries.push({ key: 'Model access', data: props.accessLevel });
  if (props.contractEnforced != null)
    entries.push({
      key: 'Contract enforced',
      data: props.contractEnforced ? 'true' : 'false',
    });
  if (props.materialization)
    entries.push({ key: 'Materialization type', data: props.materialization });
  if (props.group) entries.push({ key: 'Group', data: props.group });
  if (props.tags?.length) entries.push({ key: 'Tags', data: props.tags.join(', ') });
  if (props.meta)
    entries.push({
      key: 'Meta',
      data: <code>{JSON.stringify(props.meta, null, 2)}</code>,
    });
  return entries;
}

function sourceToEntries(props: AssetMetadataProps): Entry[] {
  const entries: Entry[] = [];
  if (props.loader) entries.push({ key: 'Loader', data: props.loader });
  if (props.relation?.database)
    entries.push({ key: 'Database', data: props.relation.database });
  if (props.relation?.schema)
    entries.push({ key: 'Schema', data: props.relation.schema });
  if (props.meta)
    entries.push({
      key: 'Meta',
      data: <code>{JSON.stringify(props.meta, null, 2)}</code>,
    });
  return entries;
}

function exposureToEntries(props: AssetMetadataProps): Entry[] {
  const entries: Entry[] = [];
  if (props.exposureType)
    entries.push({ key: 'Exposure type', data: props.exposureType });
  if (props.maturity) entries.push({ key: 'Maturity', data: props.maturity });
  if (props.url) entries.push({ key: 'URL', data: props.url });
  if (props.ownerName) entries.push({ key: 'Owner', data: props.ownerName });
  if (props.ownerEmail) entries.push({ key: 'Owner email', data: props.ownerEmail });
  if (props.tags?.length) entries.push({ key: 'Tags', data: props.tags.join(', ') });
  if (props.packageName) entries.push({ key: 'Package', data: props.packageName });
  if (props.filePath) entries.push({ key: 'File', data: props.filePath });
  return entries;
}

function macroToEntries(props: AssetMetadataProps): Entry[] {
  const entries: Entry[] = [];
  if (props.packageName) entries.push({ key: 'Package', data: props.packageName });
  if (props.filePath) entries.push({ key: 'File', data: props.filePath });
  return entries;
}

function semanticModelToEntries(props: AssetMetadataProps): Entry[] {
  const entries: Entry[] = [];
  if (props.packageName) entries.push({ key: 'Package', data: props.packageName });
  if (props.tags?.length) entries.push({ key: 'Tags', data: props.tags.join(', ') });
  return entries;
}

function savedQueryToEntries(props: AssetMetadataProps): Entry[] {
  const entries: Entry[] = [];
  if (props.packageName) entries.push({ key: 'Package', data: props.packageName });
  if (props.tags?.length) entries.push({ key: 'Tags', data: props.tags.join(', ') });
  if (props.group) entries.push({ key: 'Group', data: props.group });
  return entries;
}

function defaultToEntries(props: AssetMetadataProps): Entry[] {
  const entries: Entry[] = [];
  if (props.packageName) entries.push({ key: 'Package', data: props.packageName });
  return entries;
}

function assetToEntries(props: AssetMetadataProps): Entry[] {
  switch (props.resourceType) {
    case 'model':
    case 'seed':
    case 'snapshot':
      return modelToEntries(props);
    case 'source':
      return sourceToEntries(props);
    case 'exposure':
      return exposureToEntries(props);
    case 'macro':
      return macroToEntries(props);
    case 'semantic_model':
      return semanticModelToEntries(props);
    case 'saved_query':
      return savedQueryToEntries(props);
    default:
      return defaultToEntries(props);
  }
}

// Query-history row only makes sense on data resources where consumption
// would be tracked — model, seed, snapshot. The UpgradeRow itself filters
// by the (kind, userState) registry, so via-catalog is suppressed there.
function showsQueryHistoryRow(props: AssetMetadataProps): boolean {
  return (
    props.userState != null &&
    (props.resourceType === 'model' ||
      props.resourceType === 'seed' ||
      props.resourceType === 'snapshot')
  );
}

/** Whether AssetMetadata would render any content. Lets callers hide an empty
 *  wrapping section (e.g. a DetailsSection heading) when there's nothing to show. */
export function hasAssetMetadata(props: AssetMetadataProps): boolean {
  return assetToEntries(props).length > 0 || showsQueryHistoryRow(props);
}

export function AssetMetadata(props: AssetMetadataProps) {
  const entries = assetToEntries(props);
  const showQueryHistoryRow = showsQueryHistoryRow(props);
  return (
    <ColumnTable
      tableEntries={entries}
      isLoading={props.isLoading}
      compact={props.compact}
      footer={
        showQueryHistoryRow && props.userState ? (
          <UpgradeRow
            label="Consumption queries (excludes builds)"
            kind="queryHistory"
            userState={props.userState}
            decorateOutboundHref={props.decorateOutboundHref}
            testId="asset-metadata-query-history-upsell"
          />
        ) : null
      }
    />
  );
}
