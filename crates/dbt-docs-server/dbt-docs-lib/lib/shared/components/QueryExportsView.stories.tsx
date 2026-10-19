import type { Meta, StoryObj } from '@storybook/react-vite';

import { QueryExportsView } from './QueryExportsView';

const meta: Meta<typeof QueryExportsView> = {
  component: QueryExportsView,
  args: {
    exports: [
      {
        name: 'weekly_revenue',
        config: {
          alias: 'weekly_revenue',
          exportAs: 'table',
          schema: 'reporting',
          database: 'analytics',
        },
      },
    ],
  },
  decorators: [(Story) => <div className="max-w-2xl">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof QueryExportsView>;

export const Default: Story = {};

export const MultipleExports: Story = {
  args: {
    exports: [
      {
        name: 'weekly_revenue',
        config: {
          alias: 'weekly_revenue',
          exportAs: 'table',
          schema: 'reporting',
          database: 'analytics',
        },
      },
      {
        name: 'weekly_revenue_vw',
        config: {
          alias: 'weekly_revenue_vw',
          exportAs: 'view',
          schema: 'reporting',
          database: 'analytics',
        },
      },
    ],
  },
};

/** A partial config: the table drops rows with no value, so this renders two rows
 *  rather than four blanks. */
export const PartialConfig: Story = {
  args: {
    exports: [{ name: 'cache_only', config: { exportAs: 'cache' } }],
  },
};

/** `null` means "still loading" here, while `[]` means "none" — the two absences are
 *  deliberately different messages. */
export const Loading: Story = {
  args: { exports: null },
};

export const NoExports: Story = {
  args: { exports: [] },
};
