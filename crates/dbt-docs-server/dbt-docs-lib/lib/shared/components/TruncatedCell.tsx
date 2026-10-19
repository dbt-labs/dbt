import { FC, isValidElement, ReactElement } from 'react';
import toast from 'react-hot-toast';
import { type CellContext, type ColumnDef } from '@tanstack/react-table';
import { Copy } from 'lucide-react';
import { twJoin } from 'tailwind-merge';

import { Tooltip } from '../../components/ui/Tooltip';
import { formatAbsoluteLocalDate, formatDateCommon } from '../util/dateUtils';

/**
 * tanstack-table types a column's `cell` renderer against `CellContext<TData, unknown>`
 * whenever the column array isn't built with `createColumnHelper` (which would otherwise
 * infer `TValue` per column). Cast a specifically-typed cell renderer at each call site
 * with this helper instead of widening the renderer's own prop type to `unknown` -- that
 * keeps the renderer itself fully type-checked for any correctly-typed accessor.
 */
export function asCellRenderer<TData extends object>(
  Component: FC<CellContext<TData, any>>,
): NonNullable<ColumnDef<TData>['cell']> {
  return Component as NonNullable<ColumnDef<TData>['cell']>;
}

type TruncatedCellProps<TData extends object> = CellContext<TData, ReactElement>;

const TruncatedCellImpl = ({
  className,
  innerClassName,
  value,
  tooltipContent,
  onClick,
}: {
  className?: string;
  innerClassName?: string;
  value: ReactElement | string;
  tooltipContent?: ReactElement | string;
  onClick?: React.MouseEventHandler<HTMLDivElement> | undefined;
}) => {
  const content =
    tooltipContent || isValidElement(value) ? value : value?.toLocaleString();
  return (
    <Tooltip
      displayOnlyWhenTruncated
      content={content}
      className={twJoin(className, 'w-full')}
      childrenAreInteractive={true}
      placement="top-end" // most of the time this is the last cell in the row
    >
      {(ref) => (
        <div ref={ref} className={twJoin('truncate', innerClassName)} onClick={onClick}>
          {value}
        </div>
      )}
    </Tooltip>
  );
};

/**
 * A cell that is truncated and displays a tooltip of the full content on hover.
 */
export const TruncatedCell = <TData extends object>({
  getValue,
}: TruncatedCellProps<TData>) => {
  const value = getValue();
  return <TruncatedCellImpl value={value} />;
};

/**
 * A cell that is truncated and displays a tooltip of the full content on hover.
 * Should set meta.align="right" to right align the header.
 */
export const RightAlignedTruncatedCell = <TData extends object>({
  getValue,
}: TruncatedCellProps<TData>) => {
  const value = getValue();
  return <TruncatedCellImpl value={value} className="text-right" />;
};

type TimestampCellValue = string | Date | null | undefined;

const TimestampCellImpl = ({ value }: { value: TimestampCellValue }) => {
  if (value === null || value === undefined || value === '') {
    return <div className="truncate">—</div>;
  }
  const date = value instanceof Date ? value : new Date(value);
  if (isNaN(date.getTime())) {
    return <div className="truncate">—</div>;
  }
  return (
    <Tooltip
      content={formatDateCommon(date).utc}
      className="w-full"
      childrenAreInteractive
      placement="top-end"
    >
      {(ref) => (
        <div ref={ref} className="truncate">
          {formatAbsoluteLocalDate(date)}
        </div>
      )}
    </Tooltip>
  );
};

type TimestampCellProps<TData extends object> = CellContext<TData, TimestampCellValue>;

/**
 * A cell that renders an absolute local date with a hover tooltip showing the
 * UTC/full time. Use with `accessorFn: (row) => row.<timestamp>`. Renders `—`
 * for empty/invalid values. Unlike {@link TruncatedCell}, the tooltip is always
 * shown on hover since it reveals the UTC time.
 */
export const TimestampCell = <TData extends object>({
  getValue,
}: TimestampCellProps<TData>) => <TimestampCellImpl value={getValue()} />;

type TruncatedCopyLinkCellProps<TData extends object> = CellContext<TData, string>;

/**
 * A cell that is truncated and displays a tooltip of the full content on hover.
 * Uses link styling. When clicked, copies content to clipboard
 */
export const TruncatedCopyLinkCell = <TData extends object>({
  getValue,
}: TruncatedCopyLinkCellProps<TData>) => {
  const value = getValue();
  return (
    <TruncatedCellImpl
      innerClassName="py-4 cursor-pointer"
      value={
        <span className="whitespace-nowrap py-4">
          <Copy className="mr-1 size-3.5 align-middle" />
          <span className="align-middle">{value}</span>
        </span>
      }
      tooltipContent={value}
      onClick={(e) => {
        e.preventDefault();
        navigator.clipboard.writeText(value);
        toast.success('Copied to clipboard', {
          id: 'copy-to-clipboard',
        });
      }}
    />
  );
};
