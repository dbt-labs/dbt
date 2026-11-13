import { FC } from 'react';

import { SavedQueryParams } from '../typings/domain/asset';
import { CollapsibleSection } from './CollapsibleSection';
import { DetailsSection } from './SectionWithCard';

type SavedQueryParamsViewProps = {
  params: SavedQueryParams | null;
};

const segmentClassName =
  'border-borderMuted -mb-[1px] mt-1 flex w-full overflow-hidden border-b p-4';

type QueryParamsSegmentProps = {
  segmentName: string;
  content: string[];
  isCode?: boolean;
};

/** Single collapsible parameter segment. Renders only when there is content. */
const QueryParamsSegment: FC<QueryParamsSegmentProps> = ({
  segmentName,
  content,
  isCode,
}) => {
  if (content.length === 0) {
    return null;
  }
  return (
    <CollapsibleSection
      closeAltText="Hide query params segment"
      expandAltText="Show query params segment"
      className={segmentClassName}
      toExpand={
        <div aria-label="queryExports" className="space-y-2 p-4">
          {content.map((item) => (
            <div key={item}>
              {isCode ? (
                <code className="rounded bg-bgNeutralMuted px-1 py-0.5 font-mono text-xs">
                  {item}
                </code>
              ) : (
                <span className="align-middle text-fgDecorative">{item}</span>
              )}
            </div>
          ))}
        </div>
      }
    >
      <span>
        <p className="mx-2 align-middle text-base font-semibold">{segmentName}</p>
      </span>
    </CollapsibleSection>
  );
};

export const SavedQueryParamsView: FC<SavedQueryParamsViewProps> = ({ params }) => {
  if (params == null) {
    return <div className="mt-10 text-fgDecorative">No parameters.</div>;
  }

  const hasMetrics = params.metrics.length > 0;
  const hasGroupBy = params.groupBy.length > 0;
  const hasWhere = params.where.length > 0;
  const hasOrderBy = params.orderBy != null && params.orderBy.length > 0;
  const hasLimit = params.limit != null;

  if (!hasMetrics && !hasGroupBy && !hasWhere && !hasOrderBy && !hasLimit) {
    return <div className="mt-10 text-fgDecorative">No parameters.</div>;
  }

  return (
    <DetailsSection heading="Parameters" withCard={false}>
      <div className="overflow-hidden rounded-md border border-borderMuted">
        <QueryParamsSegment segmentName="Metrics" content={params.metrics} />
        <QueryParamsSegment segmentName="Group By" content={params.groupBy} />
        <QueryParamsSegment segmentName="Where" content={params.where} isCode />
        <QueryParamsSegment segmentName="Order By" content={params.orderBy ?? []} />
        {hasLimit && (
          <CollapsibleSection
            disable
            closeAltText="Hide query params segment"
            expandAltText="Show query params segment"
            className={segmentClassName}
            toExpand={null}
          >
            <span>
              <p className="mx-2 align-middle text-base font-semibold">{`Limit: ${params.limit}`}</p>
            </span>
          </CollapsibleSection>
        )}
      </div>
    </DetailsSection>
  );
};
