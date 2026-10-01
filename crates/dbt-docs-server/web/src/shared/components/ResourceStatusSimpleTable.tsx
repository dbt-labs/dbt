import type { ResourceTypeExplorer } from '../../lib/resourceType';
import type { FreshnessStatusValue } from '../typings/domain/status';
import type { TestStatusValue } from '../typings/domain/status';
import { NodeStatusIconBadge } from './NodeStatusIconBadge';
import { ResourceChip } from './ResourceChip';

export type ResourceStatusRow = {
  name: string;
  uniqueId: string;
  resourceType: string;
  status: string | null;
  statusKind: 'test' | 'freshness' | 'run';
};

export type ResourceStatusSimpleTableProps = {
  rows: ResourceStatusRow[];
  onSelect?: (uniqueId: string) => void;
};

const TEST_STATUS_VALUES: TestStatusValue[] = [
  'pass',
  'fail',
  'warn',
  'error',
  'skipped',
  'unknown',
];
const FRESHNESS_STATUS_VALUES: FreshnessStatusValue[] = [
  'pass',
  'warn',
  'error',
  'outdated',
  'unconfigured',
  'skipped',
  'unknown',
];

function toTestStatus(status: string | null): TestStatusValue {
  const s = status as TestStatusValue;
  return TEST_STATUS_VALUES.includes(s) ? s : 'unknown';
}

function toFreshnessStatus(status: string | null): FreshnessStatusValue {
  const s = status as FreshnessStatusValue;
  return FRESHNESS_STATUS_VALUES.includes(s) ? s : 'unknown';
}

export const ResourceStatusSimpleTable = ({
  rows,
  onSelect,
}: ResourceStatusSimpleTableProps) => {
  return (
    <div className="border-t border-borderMuted bg-bgBackground">
      {rows.map((row) => {
        const statusBadge =
          row.statusKind === 'test' ? (
            <NodeStatusIconBadge kind="test" status={toTestStatus(row.status)} />
          ) : row.statusKind === 'freshness' ? (
            <NodeStatusIconBadge
              kind="freshness"
              status={toFreshnessStatus(row.status)}
            />
          ) : null;

        const nameContent = onSelect ? (
          <button
            type="button"
            className="truncate text-left text-sm text-fgBrand hover:underline"
            onClick={() => onSelect(row.uniqueId)}
          >
            {row.name}
          </button>
        ) : (
          <span className="truncate text-sm text-fgMain">{row.name}</span>
        );

        return (
          <div
            key={row.uniqueId}
            className="flex items-center gap-3 border-b border-borderMuted px-4 py-3 last:border-b-0"
          >
            <div className="flex-shrink-0">
              <ResourceChip
                resourceType={row.resourceType as ResourceTypeExplorer}
                showText={false}
              />
            </div>
            <div className="min-w-0 flex-1">{nameContent}</div>
            {statusBadge && <div className="flex-shrink-0">{statusBadge}</div>}
          </div>
        );
      })}
    </div>
  );
};
