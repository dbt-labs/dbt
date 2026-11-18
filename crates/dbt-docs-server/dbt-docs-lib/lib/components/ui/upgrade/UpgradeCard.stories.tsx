import type { Meta, StoryObj } from '@storybook/react-vite';

import { UpgradeCard } from './UpgradeCard';

const meta: Meta<typeof UpgradeCard> = {
  component: UpgradeCard,
  args: { kind: 'mesh', userState: 'core', variant: 'inline' },
  decorators: [(Story) => <div className="w-[640px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof UpgradeCard>;

/** `inline` — the wide single-row form used by the status panel and the
 *  asset-detail banner. */
export const Inline: Story = {};

/** `block` — the large two-row form used by the home page's persistent dbt State
 *  card. It prefers `headline` over `title` and puts the learn-more link inline in the
 *  body. */
export const Block: Story = {
  args: { kind: 'dbtState', variant: 'block' },
};

/** `rail-expanded` — the open sidebar card. */
export const RailExpanded: Story = {
  args: { variant: 'rail-expanded' },
  decorators: [(Story) => <div className="w-72">{Story()}</div>],
};

/** `rail-collapsed` — the closed sidebar card: subtitle plus a caret. */
export const RailCollapsed: Story = {
  args: { variant: 'rail-collapsed', onToggleExpand: () => {} },
  decorators: [(Story) => <div className="w-72">{Story()}</div>],
};

/**
 * Column lineage on Core sends the user to the dbt v2 install docs, because
 * `dbt login` does not exist on Core. On `proprietary-anon` the same card offers the
 * `dbt login` snippet instead — the clearest example of the copy registry changing the
 * CTA *kind*, not just its wording.
 */
export const ColumnLineageOnCore: Story = {
  args: { kind: 'columnLineage', userState: 'core' },
};

export const ColumnLineageOnProprietaryAnon: Story = {
  args: { kind: 'columnLineage', userState: 'proprietary-anon' },
};

/** Once logged in, column lineage is already on: the card renders its on-state (green
 *  dot, "is ON" title) and no CTA at all. */
export const ColumnLineageAlreadyOn: Story = {
  args: { kind: 'columnLineage', userState: 'proprietary-logged-in' },
};

/** A hidden cell renders nothing — mesh has nothing to say to a platform user. */
export const HiddenCellRendersNothing: Story = {
  args: { kind: 'mesh', userState: 'via-catalog' },
};

/** Passing `onDismiss` is what makes the X appear; persisting the dismissal is the
 *  caller's job. */
export const Dismissable: Story = {
  args: { variant: 'rail-expanded', onDismiss: () => {} },
  decorators: [(Story) => <div className="w-72">{Story()}</div>],
};

/** Every kind in the `inline` variant for one user state — the fastest way to review
 *  the copy registry as a set. */
export const AllKindsInline: Story = {
  render: () => (
    <div className="space-y-3">
      {(['dbtState', 'columnLineage', 'mesh', 'queryHistory'] as const).map((kind) => (
        <UpgradeCard key={kind} kind={kind} userState="core" variant="inline" />
      ))}
    </div>
  ),
};

/** The same kind across every user state, to see the gating. */
export const OneKindAcrossUserStates: Story = {
  render: () => (
    <div className="space-y-3">
      {(
        ['core', 'proprietary-anon', 'proprietary-logged-in', 'via-catalog'] as const
      ).map((userState) => (
        <div key={userState}>
          <p className="mb-1 text-xs uppercase text-fgDecorative">{userState}</p>
          <UpgradeCard kind="columnLineage" userState={userState} variant="inline" />
        </div>
      ))}
    </div>
  ),
};
