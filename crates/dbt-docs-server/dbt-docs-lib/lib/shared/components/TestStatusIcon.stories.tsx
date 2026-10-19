import type { Meta, StoryObj } from '@storybook/react-vite';

import type { TestStatus } from '../util/testStatus';
import { TestStatusIcon } from './TestStatusIcon';

const meta: Meta<typeof TestStatusIcon> = {
  component: TestStatusIcon,
  args: { status: 'pass' },
};

export default meta;
type Story = StoryObj<typeof TestStatusIcon>;

export const Default: Story = {};

/** Note `error` and `fail` share an icon and colour deliberately — the distinction
 *  matters to dbt, not to a reader scanning a list. */
export const AllStatuses: Story = {
  render: () => (
    <div className="flex items-center gap-4">
      {(['pass', 'fail', 'error', 'warn', 'skipped', 'reused'] as TestStatus[]).map(
        (status) => (
          <span key={status} className="flex items-center gap-1 text-sm text-fgAlt">
            <TestStatusIcon status={status} />
            {status}
          </span>
        ),
      )}
    </div>
  ),
};

/** An unmapped status renders nothing rather than a placeholder glyph. */
export const UnmappedStatusRendersNothing: Story = {
  args: { status: 'unknown' as TestStatus },
};

export const WithCustomClass: Story = {
  args: { className: 'h-8 w-8' },
};
