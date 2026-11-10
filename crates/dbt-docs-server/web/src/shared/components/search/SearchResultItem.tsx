import { FC } from 'react';

import { Link } from '../../../components/ui/Link';
import { LoadingBlock } from '../../../components/ui/LoadingBlock';
import type { ResourceType, ResourceTypeExplorer } from '../../../lib/resourceType';
import { TrustSignals } from '../../typings/trustSignals';
import { ResourceChip } from '../ResourceChip';
import { TrustSignalsBadgeContainer } from '../TrustSignalsBadge';
import { BOLD_TAG_REGEX, BoldedText, BoldSearchHighlight } from './BoldedText';
import { MatchedField, SearchResultDisplayData } from './types';

type SkeletonItemParams = {
  data?: never;
  query?: never;
  testId?: string;
  skeleton: true;
  getResourceHref?: never;
  getColumnHref?: never;
  getLineageHref?: never;
  trustSignals?: never;
};

type SearchResultItemParams = {
  data: SearchResultDisplayData;
  query: string;
  testId?: string;
  skeleton?: never;
  /** Builds the route to the resource detail page. */
  getResourceHref: (uniqueId: string) => string;
  /**
   * Builds the route to a specific column on the resource detail page. When
   * omitted, matched columns render as plain bold text instead of links.
   */
  getColumnHref?: (uniqueId: string, columnName: string) => string;
  /**
   * Builds the route for the "View lineage" CTA. When omitted, no CTA is
   * rendered, regardless of whether the hit has an `fqn`.
   */
  getLineageHref?: (uniqueId: string, fqn: string[]) => string;
  /**
   * Optional trust-signals payload. When omitted, no badge is rendered —
   * appropriate for consumers (e.g. docs-v2) that have no health-issue data.
   */
  trustSignals?: TrustSignals;
};

const possiblyTruncate = (matchedField: MatchedField, text: string): string => {
  switch (matchedField) {
    case 'description':
      return `...${text}...`;
    case 'column': {
      const truncatedColumns = text.split(', ').slice(0, 5).join(', ').toUpperCase();
      if (truncatedColumns.length < text.length) {
        return `${truncatedColumns}...`;
      }
      return truncatedColumns;
    }
    default:
      return text;
  }
};

type SecondLineProps = {
  data: SearchResultDisplayData;
  query: string;
  getColumnHref?: (uniqueId: string, columnName: string) => string;
  uniqueId: string;
};

const SecondLine: FC<SecondLineProps> = ({ data, query, getColumnHref, uniqueId }) => {
  if (!data.highlight) return null;
  const truncatedText = possiblyTruncate(data.matchedField, data.highlight);
  const isColumnMatch = data.matchedField === 'column';
  const highlightedColumns = truncatedText.split(', ');

  return (
    <>
      Includes {data.matchedField}:{' '}
      {isColumnMatch &&
        highlightedColumns.map((column, index) => {
          const columnName = column.replace(BOLD_TAG_REGEX, '');
          const highlight = (
            <BoldSearchHighlight key={`hl-${index}`} text={column} query={query} />
          );
          return (
            <span key={index}>
              {index > 0 && ', '}
              {getColumnHref ? (
                <Link to={getColumnHref(uniqueId, columnName)} isInternal>
                  {highlight}
                </Link>
              ) : (
                highlight
              )}
            </span>
          );
        })}
      {!isColumnMatch && <BoldSearchHighlight text={truncatedText} query={query} />}
    </>
  );
};

const SKELETON_HEIGHT_PX = 60.67;

/**
 * One row in a project-search results list. Renders a resource link plus
 * optional trust-signals badge, optional "View lineage" CTA, and a second
 * line summarising the matched field/highlight when relevant.
 *
 * Cloud-routing-free: link targets come from caller-supplied builders so the
 * same component can be reused by dbt-explorer (Cloud routing) and docs-v2
 * (local single-project routing).
 */
export const SearchResultItem: FC<SearchResultItemParams | SkeletonItemParams> = (
  props,
) => {
  const { testId, skeleton } = props;

  if (skeleton) {
    return (
      <li
        key="skeleton"
        aria-label="Loading..."
        data-testid={`skeleton-${testId}`}
        className="list-none overflow-hidden rounded-lg border border-borderMuted text-sm"
      >
        <LoadingBlock width={-1} height={SKELETON_HEIGHT_PX} />
      </li>
    );
  }

  const { data, query, getResourceHref, getColumnHref, getLineageHref, trustSignals } =
    props;

  if (!data || data.hit?.name == null) {
    return null;
  }

  const hit = data.hit;
  const displayText =
    hit.resourceType === 'model' || hit.resourceType === 'source'
      ? uniqueIdRemainder(hit.uniqueId)
      : hit.name || '';
  const shouldShowSecondLine = data.matchedField !== 'name' && !!data.highlight;
  const renderLineageLink = !!getLineageHref && !!hit.fqn;

  return (
    <li
      key={hit.name || ''}
      aria-label={hit.name || ''}
      data-testid={testId}
      className="list-none overflow-hidden rounded-lg border border-borderMuted text-sm"
    >
      <div className="bg-bgMain p-4">
        <div className="flex w-full items-center gap-2 overflow-hidden">
          <div className="flex-1 items-center overflow-hidden truncate">
            <span className="flex truncate">
              <Link
                className="pr-2 text-base"
                isInternal
                to={getResourceHref(hit.uniqueId)}
              >
                <BoldedText
                  text={displayText}
                  shouldBeBold={query}
                  boldProps={{ className: 'bg-bgBrandMuted' }}
                />
              </Link>
              {trustSignals && (
                <TrustSignalsBadgeContainer
                  resourceType={hit.resourceType as ResourceType}
                  trustSignals={trustSignals}
                />
              )}
            </span>
          </div>
          {renderLineageLink && hit.fqn && (
            <span className="flex items-center text-sm">
              <Link
                className="text-base"
                isInternal
                hideUnderline
                to={getLineageHref(hit.uniqueId, hit.fqn)}
              >
                View lineage
              </Link>
            </span>
          )}
          <span className="flex-0 text-sm">
            <ResourceChip resourceType={hit.resourceType as ResourceTypeExplorer} />
          </span>
        </div>
        {shouldShowSecondLine && (
          <div className="overflow-hidden truncate text-fgDecorative">
            <SecondLine
              data={data}
              query={query}
              getColumnHref={getColumnHref}
              uniqueId={hit.uniqueId}
            />
          </div>
        )}
      </div>
    </li>
  );
};

/**
 * Strips the `{resourceType}.{package}.` prefix from a uniqueId, leaving the
 * remainder (model name plus any version suffix, e.g. `my_model.v2`). Falls
 * back to the original uniqueId when the input doesn't have the expected
 * 3-segment shape.
 */
function uniqueIdRemainder(uniqueId: string): string {
  const parts = uniqueId.split('.');
  if (parts.length < 3) return uniqueId;
  return parts.slice(2).join('.');
}
