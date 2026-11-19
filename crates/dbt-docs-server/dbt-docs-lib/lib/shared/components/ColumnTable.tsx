import { FC, isValidElement } from 'react';
import { twMerge } from 'tailwind-merge';

import { Link } from '../../components/ui/Link';
import { Tooltip } from '../../components/ui/Tooltip';
import type { RunStatus } from '../typings/domain/executionInfo';
import { formatDateCommon } from '../util/dateUtils';
import { camelToTitleCase } from '../util/string';
import { NodeStatusIconBadge } from './NodeStatusIconBadge';
import { Spinner } from './Spinner';

export type Entry = {
  title?: React.ReactNode;
  key: string;
  data: React.ReactNode;
  disableTooltip?: boolean;
  link?: string;
  crossProjectLink?: boolean;
};

type DataTableParams = {
  tableEntries: Entry[] | undefined | null;
  isLoading: boolean | undefined;
  compact?: boolean;
  testId?: string;
  /** Optional trailing row rendered inside the same divide-y wrapper, so it
   *  picks up the row divider and spacing rhythm of the table above it. */
  footer?: React.ReactNode;
};

type ColumnTableRowParams = {
  entry: Entry;
  compact?: boolean;
  testId?: string;
};

const ColumnTableDataCell = ({ entry }: ColumnTableRowParams) => {
  if (entry.key === 'executionInfo.lastRunStatus') {
    return <NodeStatusIconBadge kind="run" status={entry.data as RunStatus} />;
  }
  if (entry.key === 'executionInfo.executeCompletedAt' && entry.data) {
    return <>{formatDateCommon(new Date(entry.data?.toString())).humanized}</>;
  }
  if (isValidElement(entry.data)) {
    return entry.data;
  }

  if (Array.isArray(entry.data) && entry.data.length > 0) {
    return <>`[${entry.data.join(', ')}]`</>;
  }
  return <>{entry.data}</>;
};

const ColumnTableRow = ({
  entry,
  compact,
  testId = 'column-table-row',
}: ColumnTableRowParams) => {
  const marginY = compact ? 'my-2' : 'my-5';
  const fontSize = compact ? 'text-sm' : '';
  const tooltipContent =
    !isValidElement(entry.data) && typeof entry.data === 'object'
      ? JSON.stringify(entry.data)
      : entry.data;
  const title = entry.title ?? entry.key;
  return (
    <div className="flex flex-wrap" data-testid={testId}>
      <span className={twMerge('flex w-full', marginY)}>
        <span
          className={twMerge(
            'min-w-[140px] flex-1 truncate whitespace-nowrap text-fgDecorative',
            fontSize,
          )}
          data-testid={`${testId}-title`}
        >
          {typeof title === 'string' ? camelToTitleCase(title) : title}
        </span>
        {entry.disableTooltip && (
          <span
            data-testid={`${testId}-entry`}
            className={twMerge(
              'flex-0 block overflow-hidden whitespace-nowrap pl-3',
              fontSize,
            )}
          >
            <ColumnTableDataCell entry={entry} />
          </span>
        )}
        {!entry.disableTooltip && (
          <span
            data-testid={`${testId}-entry`}
            className={twMerge(
              'flex-0 overflow-hidden whitespace-nowrap pl-3',
              fontSize,
            )}
          >
            <Tooltip
              displayOnlyWhenTruncated
              content={tooltipContent}
              placement="top-end"
            >
              {(ref) => (
                <>
                  {entry.link && (
                    <Link
                      isInternal
                      to={entry.link}
                      target={entry.crossProjectLink ? '_blank' : ''}
                    >
                      <span ref={ref} className="block truncate">
                        <ColumnTableDataCell entry={entry} />
                      </span>
                    </Link>
                  )}
                  {!entry.link && (
                    <span ref={ref} className="block truncate">
                      <ColumnTableDataCell entry={entry} />
                    </span>
                  )}
                </>
              )}
            </Tooltip>
          </span>
        )}
      </span>
    </div>
  );
};

export const ColumnTable: FC<DataTableParams> = ({
  tableEntries,
  compact,
  isLoading,
  testId = 'column-table',
  footer,
}) => {
  if (isLoading) return <Spinner />;
  if (!tableEntries?.length && !footer) return null;
  const extraClasses = compact ? '' : 'mx-6 divide-y divide-borderMuted';
  return (
    <div className={twMerge(`my-2 overflow-hidden`, extraClasses)} data-testid={testId}>
      {(tableEntries ?? [])
        .filter(
          (x) =>
            x.data !== '' &&
            x.data != null &&
            (!Array.isArray(x.data) || x.data?.length !== 0),
        )
        .map((entry, idx) => {
          return (
            <ColumnTableRow
              key={entry.key}
              entry={entry}
              compact={compact}
              testId={`${testId}-${idx}`}
            />
          );
        })}
      {footer}
    </div>
  );
};
