import type { Meta, StoryObj } from '@storybook/react-vite';

import { SelectorLink } from './SelectorLink';

const meta: Meta<typeof SelectorLink> = {
  component: SelectorLink,
  args: { selector: 'customers+', label: 'View downstream lineage' },
};

export default meta;
type Story = StoryObj<typeof SelectorLink>;

/** Links to `?select=<selector>`, which the lineage page reads. */
export const Default: Story = {};

/** dbt selector syntax is full of characters that need escaping — this is the story
 *  that shows the `encodeURIComponent` actually happening. Inspect the href. */
export const SelectorNeedingEscaping: Story = {
  args: {
    selector: 'tag:daily,config.materialized:incremental+',
    label: 'View daily incrementals',
  },
};

export const UpstreamAndDownstream: Story = {
  args: { selector: '+customers+', label: 'View full lineage' },
};
