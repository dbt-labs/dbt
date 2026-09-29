import type { Meta, StoryObj } from '@storybook/react-vite';

import type { RunStatus } from '../typings/domain/executionInfo';
import type { FreshnessStatusValue, TestStatusValue } from '../typings/domain/status';
import { NodeStatusIconBadge } from './NodeStatusIconBadge';

const meta: Meta<typeof NodeStatusIconBadge> = {
  component: NodeStatusIconBadge,
  args: { kind: 'run', status: 'success' },
};

export default meta;
type Story = StoryObj<typeof NodeStatusIconBadge>;

export const Default: Story = {};

/** The three vocabularies share result names but not colours or icons — `warn` is a
 *  test/freshness concept, `reused` a run one. Reviewing them together is the point. */
export const RunStatuses: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      {(
        ['success', 'error', 'running', 'queued', 'skipped', 'reused'] as RunStatus[]
      ).map((status) => (
        <NodeStatusIconBadge key={status} kind="run" status={status} />
      ))}
    </div>
  ),
};

export const TestStatuses: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      {(
        ['pass', 'fail', 'warn', 'error', 'skipped', 'unknown'] as TestStatusValue[]
      ).map((status) => (
        <NodeStatusIconBadge key={status} kind="test" status={status} />
      ))}
    </div>
  ),
};

export const FreshnessStatuses: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      {(
        [
          'pass',
          'warn',
          'error',
          'outdated',
          'unconfigured',
          'skipped',
          'unknown',
        ] as FreshnessStatusValue[]
      ).map((status) => (
        <NodeStatusIconBadge key={status} kind="freshness" status={status} />
      ))}
    </div>
  ),
};

/** Two statusless kinds. `runtimeError` is for a failure in the UI itself rather than
 *  in dbt; `none` is "we have no status for this node". */
export const SpecialKinds: Story = {
  render: () => (
    <div className="flex gap-2">
      <NodeStatusIconBadge kind="runtimeError" />
      <NodeStatusIconBadge kind="none" />
    </div>
  ),
};

export const Small: Story = {
  args: { size: 'small' },
};

export const WithTooltip: Story = {
  args: { tooltip: 'Last build completed at 4:12 PM' },
};

/**
 * A status outside the declared kind still renders. Callers cast across vocabularies
 * (a test's `lastRunStatus as RunStatus`), so the component falls back to any known
 * status and then to a neutral badge instead of crashing — the fix for META-7840.
 */
export const StatusOutsideItsKind: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      {/* `warn` is not in the run vocabulary; resolved from the shared fallback. */}
      <NodeStatusIconBadge kind="run" status={'warn' as RunStatus} />
      {/* Not in any vocabulary — neutral badge, no icon. */}
      <NodeStatusIconBadge kind="run" status={'invented' as RunStatus} />
      {/* An inherited Object key must not resolve to a prototype value. */}
      <NodeStatusIconBadge kind="run" status={'toString' as RunStatus} />
    </div>
  ),
};
