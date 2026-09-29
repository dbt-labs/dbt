import type { Meta, StoryObj } from '@storybook/react-vite';

import { UpgradeRailStack } from './UpgradeRailStack';

const meta: Meta<typeof UpgradeRailStack> = {
  component: UpgradeRailStack,
  args: { userState: 'core' },
  decorators: [(Story) => <div className="w-72">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof UpgradeRailStack>;

/**
 * The sidebar footer stack. Two cards at most, one expanded; clicking a collapsed
 * card swaps which is open.
 *
 * Every story below passes its own `dismissedStorageKey`. Dismissals persist to
 * localStorage for 30 days, so without separate keys dismissing a card in one story
 * would silently blank the others — including on someone else's next visit to
 * Storybook.
 */
export const Default: Story = {
  args: { dismissedStorageKey: 'sb:upgrade-rail:default' },
};

/** Passing a single kind is how the real side panel uses it — the rail only ever
 *  upsells Mesh. */
export const MeshOnly: Story = {
  args: { kinds: ['mesh'], dismissedStorageKey: 'sb:upgrade-rail:mesh-only' },
};

/** `defaultOpenKey: null` starts every card collapsed. */
export const StartsCollapsed: Story = {
  args: {
    defaultOpenKey: null,
    dismissedStorageKey: 'sb:upgrade-rail:collapsed',
  },
};

export const OpensASpecificCard: Story = {
  args: {
    defaultOpenKey: 'mesh',
    dismissedStorageKey: 'sb:upgrade-rail:specific',
  },
};

/** Raising the cap shows more of the pool at once; the default of two comes from the
 *  design handoff. */
export const MoreVisibleCards: Story = {
  args: { maxVisible: 4, dismissedStorageKey: 'sb:upgrade-rail:four' },
};

/**
 * `via-catalog` hides every kind except dbt State, so the stack filters down to one
 * card rather than rendering empty slots.
 */
export const ViaCatalog: Story = {
  args: {
    userState: 'via-catalog',
    dismissedStorageKey: 'sb:upgrade-rail:via-catalog',
  },
};

/** No visible kinds renders nothing at all, so the sidebar footer collapses. */
export const NoVisibleKindsRendersNothing: Story = {
  args: {
    kinds: ['mesh', 'queryHistory'],
    userState: 'via-catalog',
    dismissedStorageKey: 'sb:upgrade-rail:none',
  },
};

export const ProprietaryLoggedIn: Story = {
  args: {
    userState: 'proprietary-logged-in',
    dismissedStorageKey: 'sb:upgrade-rail:logged-in',
  },
};
