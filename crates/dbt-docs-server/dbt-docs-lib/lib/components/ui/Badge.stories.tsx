import type { Meta, StoryObj } from '@storybook/react-vite';
import { CircleCheck, CircleX, TriangleAlert } from 'lucide-react';

import { Badge } from './Badge';

/**
 * A non-interactive label. `text` is a string rather than children, so a badge can
 * never grow into a layout of its own — anything richer belongs in a `Pill` (which is
 * removable) or a `NotificationBanner`.
 */
const meta: Meta<typeof Badge> = {
  component: Badge,
  args: { text: 'materialized: table' },
};

export default meta;
type Story = StoryObj<typeof Badge>;

/** The default variant is `secondary`, not `default` — neutral grey is what a metadata
 *  chip wants, and the indigo `default` variant is reserved for emphasis. */
export const Default: Story = {};

export const AllVariants: Story = {
  render: () => (
    <div className="flex flex-wrap items-center gap-2">
      <Badge text="default" variant="default" />
      <Badge text="secondary" variant="secondary" />
      <Badge text="destructive" variant="destructive" />
      <Badge text="outline" variant="outline" />
    </div>
  ),
};

/** `xs` / `sm` / `lg`. All three keep `whitespace-nowrap`, so a badge widens its row
 *  rather than wrapping. */
export const AllSizes: Story = {
  render: () => (
    <div className="flex flex-wrap items-center gap-2">
      <Badge text="xs" size="xs" />
      <Badge text="sm" size="sm" />
      <Badge text="lg" size="lg" />
    </div>
  ),
};

/** With a leading icon. `icon` is a node, so the caller sizes it — `size-3` is what the
 *  app uses next to badge-sized text, and it does not scale with the badge's own
 *  `size`. */
export const WithIcon: Story = {
  render: () => (
    <div className="flex flex-wrap items-center gap-2">
      <Badge
        text="passed"
        variant="default"
        icon={<CircleCheck className="size-3" />}
      />
      <Badge
        text="stale"
        variant="secondary"
        icon={<TriangleAlert className="size-3" />}
      />
      <Badge
        text="failed"
        variant="destructive"
        icon={<CircleX className="size-3" />}
      />
    </div>
  ),
};

/** How the topbar tags the site as pre-release: the smallest size, indigo, next to the
 *  project name. */
export const BetaTag: Story = {
  args: { text: 'beta', variant: 'default', size: 'xs' },
  render: (args) => (
    <div className="flex items-center gap-2 text-sm text-fgMain">
      jaffle_shop
      <Badge {...args} />
    </div>
  ),
};

/** Config values are the most common content, and several in a row is the normal case
 *  — this is roughly what a model detail header renders. */
export const MetadataRow: Story = {
  render: () => (
    <div className="flex flex-wrap items-center gap-2">
      <Badge text="model" variant="outline" size="xs" />
      <Badge text="materialized: incremental" size="xs" />
      <Badge text="tags: nightly" size="xs" />
      <Badge text="owner: analytics" size="xs" />
    </div>
  ),
};

/** `className` is merged last by `cn`, so a caller can override the variant's own
 *  colours — used sparingly, for one-off states the variants don't cover. */
export const ClassNameOverride: Story = {
  args: { text: 'custom', className: 'bg-bgBadgeIndigoMuted text-fgBrand uppercase' },
};
