import type { Meta, StoryObj } from '@storybook/react-vite';

import { AutoExposureChip } from './AutoExposureChip';

const meta: Meta<typeof AutoExposureChip> = {
  component: AutoExposureChip,
  args: { biProvider: 'tableau' },
};

export default meta;
type Story = StoryObj<typeof AutoExposureChip>;

export const Tableau: Story = {};

/** The other provider, and the reason the name is mapped rather than title-cased:
 *  "powerbi" has to render as "Power BI". */
export const PowerBi: Story = {
  args: { biProvider: 'powerbi' },
};

/** No provider renders nothing — auto-exposures are a gated capability, so the absent case
 *  has to disappear cleanly rather than leave an empty chip. */
export const NoProviderRendersNothing: Story = {
  args: { biProvider: null },
};
