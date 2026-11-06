import type { Meta, StoryObj } from '@storybook/react-vite';

import { DataPlatformChip } from './DataPlatformChip';

const meta: Meta<typeof DataPlatformChip> = {
  component: DataPlatformChip,
  args: { platform: 'snowflake' },
};

export default meta;
type Story = StoryObj<typeof DataPlatformChip>;

export const Default: Story = {};

/** Every warehouse plus dbt itself, which renders nothing — no branded mark,
 *  no other platform to name. */
export const AllPlatforms: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      {(['dbt', 'snowflake', 'databricks', 'bigquery', 'redshift'] as const).map(
        (platform) => (
          <DataPlatformChip key={platform} platform={platform} />
        ),
      )}
    </div>
  ),
};

/** Borderless is how `ResourceChip` renders it — the chip supplies its own
 *  background instead. */
export const Borderless: Story = {
  args: { bordered: false },
};
