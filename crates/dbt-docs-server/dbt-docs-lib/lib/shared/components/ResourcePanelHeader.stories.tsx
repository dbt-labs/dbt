import type { Meta, StoryObj } from '@storybook/react-vite';
import { Copy, GitBranch } from 'lucide-react';

import { Button } from '../../components/ui/Button';
import { DataPlatformChip } from './DataPlatformChip';
import { ResourcePanelHeader } from './ResourcePanelHeader';

const meta: Meta<typeof ResourcePanelHeader> = {
  component: ResourcePanelHeader,
  args: {
    resourceType: 'model',
    actions: (
      <>
        <Button
          variant="ghost"
          size="icon-sm"
          icon={<GitBranch className="size-4" />}
          ariaLabel="View lineage"
          tooltip="View lineage"
        />
        <Button
          variant="ghost"
          size="icon-sm"
          icon={<Copy className="size-4" />}
          ariaLabel="Copy link"
          tooltip="Copy link"
        />
      </>
    ),
  },
  decorators: [
    (Story) => (
      <div className="w-[420px] rounded-lg border border-borderMuted">{Story()}</div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof ResourcePanelHeader>;

/** The bar at the top of the peek panel: type chip on the left, actions right. */
export const Default: Story = {};

export const Source: Story = {
  args: { resourceType: 'source' },
};

/** `chip` replaces the default `ResourceChip` — used when the panel wants to say
 *  something other than the resource type. */
export const CustomChip: Story = {
  args: { chip: <DataPlatformChip platform="snowflake" /> },
};

/** No actions still keeps the bar and its bottom border, so the panel below does not
 *  shift. */
export const WithoutActions: Story = {
  args: { actions: null },
};
