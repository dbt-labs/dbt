import type { Meta, StoryObj } from '@storybook/react-vite';

import { UpgradeRow } from './UpgradeRow';

const meta: Meta<typeof UpgradeRow> = {
  component: UpgradeRow,
  args: {
    label: 'Consumption queries (excludes builds)',
    kind: 'queryHistory',
    userState: 'core',
  },
  decorators: [(Story) => <div className="max-w-2xl">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof UpgradeRow>;

/** A row standing in for a field the user does not have — it looks like the
 *  metadata rows around it, with a CTA where the value would be. */
export const Default: Story = {};

/** The copy is the same for every state that shows the row; what differs is whether
 *  it shows at all. */
export const ProprietaryLoggedIn: Story = {
  args: { userState: 'proprietary-logged-in' },
};

/**
 * `via-catalog` resolves to hidden copy for query history — the platform already has
 * it, so there is nothing to upsell. Renders nothing.
 */
export const HiddenForViaCatalog: Story = {
  args: { userState: 'via-catalog' },
};

/**
 * The row only renders `button` CTAs. Column lineage on `proprietary-anon` resolves to
 * a *snippet* CTA, which does not fit a dense row, so the row drops out — the reason
 * the component filters on CTA kind and not just on visibility.
 */
export const HiddenForSnippetCta: Story = {
  args: {
    kind: 'columnLineage',
    userState: 'proprietary-anon',
    label: 'Column lineage',
  },
};

export const MeshKind: Story = {
  args: { kind: 'mesh', label: 'Cross-project references' },
};

/** `decorateOutboundHref` rewrites the destination at click time — how a consumer
 *  attaches referral parameters without the component knowing about them. */
export const WithDecoratedHref: Story = {
  args: {
    decorateOutboundHref: (href) => `${href}?utm_source=dbt-docs&utm_medium=upsell`,
  },
};

/** Long labels truncate rather than pushing the CTA off the row. */
export const LongLabel: Story = {
  args: {
    label:
      'Consumption queries against this model over the last 30 days, excluding builds and tests',
  },
};
