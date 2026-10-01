import type { Meta, StoryObj } from '@storybook/react-vite';

import { UpgradeStatusPanel } from './UpgradeStatusPanel';

const meta: Meta<typeof UpgradeStatusPanel> = {
  component: UpgradeStatusPanel,
  args: {
    kinds: ['dbtState', 'columnLineage', 'mesh', 'queryHistory'],
    userState: 'core',
  },
  decorators: [(Story) => <div className="w-[760px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof UpgradeStatusPanel>;

/**
 * The home page's "Get more from dbt" panel. Rows come from the copy registry and are
 * clamped to two, so the ordering of `kinds` decides what a given user sees.
 *
 * Note the panel dismissal persists to a single fixed localStorage key. If these
 * stories look empty, the panel was dismissed in an earlier session — clear
 * `dbt:upgrade-status-panel:dismissed-v1`.
 */
export const Default: Story = {};

/** `compact` drops the heading and the contact-sales button, for embedding as a
 *  banner rather than a panel. */
export const Compact: Story = {
  args: { density: 'compact' },
};

/** Reordering `kinds` changes which two survive the clamp. */
export const ReorderedKinds: Story = {
  args: { kinds: ['mesh', 'queryHistory', 'dbtState', 'columnLineage'] },
};

export const ProprietaryAnon: Story = {
  args: { userState: 'proprietary-anon' },
};

/** Logged in, column lineage is already on — its row renders the on-state rather than
 *  a CTA. */
export const ProprietaryLoggedIn: Story = {
  args: { userState: 'proprietary-logged-in' },
};

/** On `via-catalog` only dbt State survives gating, so the panel shows a single row. */
export const ViaCatalog: Story = {
  args: { userState: 'via-catalog' },
};

/** Every kind hidden for this state renders nothing. */
export const NoVisibleRowsRendersNothing: Story = {
  args: { kinds: ['mesh', 'queryHistory', 'columnLineage'], userState: 'via-catalog' },
};

export const CustomContactSalesUrl: Story = {
  args: { contactSalesUrl: 'https://example.com/talk-to-us' },
};

/** Outbound hrefs — the contact-sales button and every row CTA — pass through
 *  `decorateOutboundHref` at click time. */
export const WithDecoratedHrefs: Story = {
  args: {
    decorateOutboundHref: (href) => `${href}?utm_source=dbt-docs`,
  },
};
