import { createElement, FC, ReactNode } from 'react';
import { Columns3, Folder, Server } from 'lucide-react';
import { twJoin } from 'tailwind-merge';

import { Link } from '../../../components/ui/Link';
import { LoadingBlock } from '../../../components/ui/LoadingBlock';
import { Tooltip } from '../../../components/ui/Tooltip';
import {
  iconForType,
  type ResourceType,
  type ResourceTypeExplorer,
  type WarehouseType,
} from '../../../lib/resourceType';
import { TrustSignals } from '../../typings/trustSignals';
import { toTitleCase } from '../../util/string';
import { DataPlatformChip } from '../DataPlatformChip';
import { TrustSignalsBadgeContainer } from '../TrustSignalsBadge';
import { BoldedText } from './BoldedText';
import { HighlightPills, HighlightsByField } from './HighlightPills';
import { SearchResultDisplayData } from './types';

/** Optional second-row metadata for a single rich result row. */
export interface RichSearchResultMetadata {
  /** Pre-formatted label shown next to the resource name (e.g. "Last run: 2h ago"). */
  lastRunLabel?: string;
  /** Project or package name (rendered with the projects icon). */
  projectName?: string;
  /** Deployment environment (e.g. "Production"). Omitted apps can leave this blank. */
  environmentType?: string;
  /** Column count, rendered as "{n} columns" with the column icon. */
  numColumns?: number;
  /** Data platform shown on the left of row 1. Defaults to `'dbt'`. */
  dataPlatform?: 'dbt' | WarehouseType;
  /** Extra inline pills/badges appended after the resource-type icon (e.g. materialization). */
  extras?: ReactNode;
}

interface RichItemParams {
  data: SearchResultDisplayData;
  query: string;
  testId?: string;
  skeleton?: never;
  getResourceHref: (uniqueId: string) => string;
  /** Per-row metadata for the second row. When undefined, row 2 is suppressed. */
  metadata?: RichSearchResultMetadata;
  /** Per-row pill payload. When undefined or empty, the pills row is suppressed. */
  highlights?: HighlightsByField;
  trustSignals?: TrustSignals;
  /** Builds an in-page link to a specific column (used by column-match pills). */
  getColumnHref?: (uniqueId: string, columnName: string) => string;
}

interface SkeletonItemParams {
  data?: never;
  query?: never;
  testId?: string;
  skeleton: true;
  getResourceHref?: never;
  metadata?: never;
  highlights?: never;
  trustSignals?: never;
  getColumnHref?: never;
}

const SKELETON_HEIGHT_PX = 92;

/**
 * Strips the `{resourceType}.{package}.` prefix from a uniqueId.
 * Mirrors the helper used by {@link SearchResultItem}.
 */
function uniqueIdRemainder(uniqueId: string): string {
  const parts = uniqueId.split('.');
  if (parts.length < 3) return uniqueId;
  return parts.slice(2).join('.');
}

/**
 * Card-style search result row, lifted from dbt-explorer's account-search
 * design. Two rows of content:
 *
 *   1. Platform chip + resource name (with bolded query highlights) + trust
 *      signals badge + last-run label.
 *   2. Project name · environment · resource type · column count · extras.
 *
 * Optionally followed by a `HighlightPills` row when the hit carries non-name
 * matched fields. Link targets come from caller-supplied builders so the same
 * component can be reused by dbt-explorer (Cloud routing) and docs-v2
 * (local single-project routing).
 */
export const RichSearchResultItem: FC<RichItemParams | SkeletonItemParams> = (
  props,
) => {
  const { testId, skeleton } = props;

  if (skeleton) {
    return (
      <li
        aria-label="Loading..."
        data-testid={`skeleton-${testId}`}
        className="list-none overflow-hidden border-b border-borderMuted text-sm"
      >
        <LoadingBlock width={-1} height={SKELETON_HEIGHT_PX} />
      </li>
    );
  }

  const {
    data,
    query,
    getResourceHref,
    metadata,
    highlights,
    trustSignals,
    getColumnHref,
  } = props;

  if (!data || data.hit?.name == null) return null;

  const hit = data.hit;
  const displayText =
    hit.resourceType === 'model' || hit.resourceType === 'source'
      ? uniqueIdRemainder(hit.uniqueId)
      : hit.name || '';

  const resourceType = hit.resourceType as ResourceTypeExplorer;
  const hasMetadataRow =
    !!metadata &&
    (metadata.projectName ||
      metadata.environmentType ||
      metadata.numColumns ||
      metadata.extras ||
      // Resource type is always known — but we only render row 2 when at least
      // one *user-meaningful* field is present, to avoid a one-cell row.
      false);

  return (
    <li
      aria-label={hit.name || ''}
      data-testid={testId}
      className={twJoin(
        'list-none overflow-hidden border-b border-borderMuted text-sm',
      )}
    >
      <div className="bg-bgMain p-4">
        <div className="flex w-full items-center gap-2 overflow-hidden">
          <span className="flex-0 text-sm">
            <DataPlatformChip platform={metadata?.dataPlatform ?? 'dbt'} />
          </span>
          <div className="flex-1 items-center overflow-hidden truncate">
            <span className="flex truncate">
              <Link
                className="pr-2 text-base"
                isInternal
                to={getResourceHref(hit.uniqueId)}
              >
                <Tooltip content={hit.name ?? ''} placement="top-start">
                  <BoldedText
                    text={displayText}
                    shouldBeBold={query}
                    boldProps={{ className: 'bg-bgBrandMuted' }}
                  />
                </Tooltip>
              </Link>
              {trustSignals && (
                <TrustSignalsBadgeContainer
                  resourceType={resourceType as unknown as ResourceType}
                  trustSignals={trustSignals}
                />
              )}
            </span>
            {metadata?.lastRunLabel && (
              <span className="text-fgAlt">{metadata.lastRunLabel}</span>
            )}
          </div>
        </div>
        {(hasMetadataRow || metadata) && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 truncate pt-3 text-fgAlt">
            {metadata?.projectName && (
              <span className="flex items-center gap-1">
                <Folder className="size-3.5" />
                {metadata.projectName}
              </span>
            )}
            {metadata?.environmentType && (
              <span className="flex items-center gap-1">
                <Server className="size-3.5" />
                {toTitleCase(metadata.environmentType)}
              </span>
            )}
            <span className="flex items-center gap-1">
              {createElement(iconForType(resourceType), {
                size: 16,
              })}
              {toTitleCase(resourceType)}
            </span>
            {metadata?.numColumns != null && metadata.numColumns > 0 && (
              <span className="flex items-center gap-1">
                <Columns3 className="size-3.5" />
                {metadata.numColumns} columns
              </span>
            )}
            {metadata?.extras}
          </div>
        )}
        {highlights && (
          <HighlightPills
            highlights={highlights}
            query={query}
            getColumnHref={
              getColumnHref
                ? (column) => getColumnHref(hit.uniqueId, column)
                : undefined
            }
          />
        )}
      </div>
    </li>
  );
};
