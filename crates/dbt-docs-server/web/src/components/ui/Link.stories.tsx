import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, within } from 'storybook/test';

import { Link } from './Link';

/**
 * One component for both kinds of link, switched by `isInternal`:
 *
 * - `isInternal` renders a react-router `<Link>`, so navigation stays client-side and
 *   `to` may be an object with `pathname`/`search`/`hash`. It needs a router above it
 *   (the app is hash-routed; these stories run inside a `MemoryRouter`).
 * - Otherwise it renders a plain `<a>`, and an object `to` is flattened to its
 *   `pathname` — so build external URLs as strings.
 *
 * `shouldOpenNewTab` sets `target="_blank"` *and* the matching
 * `rel="noopener noreferrer"`, which is the reason to prefer it over passing `target`
 * yourself.
 */
const meta: Meta<typeof Link> = {
  component: Link,
  args: {
    isInternal: true,
    to: '/details/model.jaffle_shop.customers',
    children: 'customers',
  },
};

export default meta;
type Story = StoryObj<typeof Link>;

/** In-app navigation. */
export const Internal: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const link = canvas.getByRole('link', { name: 'customers' });
    await expect(link).toHaveAttribute('href', '/details/model.jaffle_shop.customers');
    // Same-tab client-side navigation: no target, and therefore no rel.
    await expect(link).not.toHaveAttribute('target');
  },
};

/** A route object, which is where the router form earns its keep — search params
 *  without hand-assembling the string. */
export const InternalWithRouteObject: Story = {
  args: {
    to: { pathname: '/resource/model', search: '?modeling_layer=marts' },
    children: 'Marts models',
  },
};

/** External documentation. */
export const External: Story = {
  args: {
    isInternal: false,
    to: 'https://docs.getdbt.com/reference/node-selection/syntax',
    children: 'selector syntax',
  },
};

/** The safe new-tab form. `noopener noreferrer` is added for you. */
export const ExternalNewTab: Story = {
  args: {
    isInternal: false,
    to: 'https://docs.getdbt.com',
    shouldOpenNewTab: true,
    children: 'dbt docs',
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const link = canvas.getByRole('link', { name: 'dbt docs' });
    await expect(link).toHaveAttribute('target', '_blank');
    await expect(link).toHaveAttribute('rel', 'noopener noreferrer');
  },
};

/** `hideUnderline` drops the underline but keeps the brand colour — for links inside a
 *  dense list or a table cell, where underlines become noise. */
export const WithoutUnderline: Story = {
  args: { hideUnderline: true },
};

/** In prose, where the underline is what distinguishes the link from emphasis. */
export const InProse: Story = {
  render: () => (
    <p className="max-w-md text-sm text-fgMain">
      This model has no lineage in the index. See{' '}
      <Link isInternal={false} to="https://docs.getdbt.com" shouldOpenNewTab>
        the docs
      </Link>{' '}
      for how to populate it, or open{' '}
      <Link isInternal to="/details/model.jaffle_shop.orders">
        orders
      </Link>{' '}
      instead.
    </p>
  ),
};

/** In a table cell: no underline until hover would be nicer, but the component has no
 *  hover-only mode — `hideUnderline` plus a hover class on `className` is the way. */
export const InTableCell: Story = {
  args: {
    hideUnderline: true,
    className: 'hover:underline',
    children: 'stg_customers',
  },
};
