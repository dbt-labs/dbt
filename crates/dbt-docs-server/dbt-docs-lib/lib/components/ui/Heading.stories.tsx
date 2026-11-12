import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, within } from 'storybook/test';

import { Heading } from './Heading';

/**
 * Type scale and heading level are separate props: `size` picks the visual step and
 * `component` picks the tag. Keeping them independent is what lets a page use one
 * `<h1>` for its title and style a small `<h2>` section label without lying about the
 * document outline.
 *
 * Note the tag is constrained to `h1`–`h6`, so the prop's doc comment about rendering
 * as a `<p>` is not expressible today.
 */
const meta: Meta<typeof Heading> = {
  component: Heading,
  args: { children: 'customers' },
};

export default meta;
type Story = StoryObj<typeof Heading>;

/** Defaults: `size="3"` rendered as an `<h2>`. */
export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole('heading', { level: 2 })).toBeInTheDocument();
  },
};

/** The whole scale, 1 (`text-4xl`) through 6 (`text-base`). */
export const AllSizes: Story = {
  render: () => (
    <div className="flex flex-col gap-2">
      <Heading size="1">size 1</Heading>
      <Heading size="2">size 2</Heading>
      <Heading size="3">size 3</Heading>
      <Heading size="4">size 4</Heading>
      <Heading size="5">size 5</Heading>
      <Heading size="6">size 6</Heading>
    </div>
  ),
};

/** A page title: the largest step, and an `<h1>` so it is also the document's top
 *  heading. */
export const PageTitle: Story = {
  args: { size: '1', component: 'h1', children: 'jaffle_shop' },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole('heading', { level: 1 })).toBeInTheDocument();
  },
};

/** A section label inside a detail page: small type, but a real `<h3>` so it nests
 *  under the page's `<h1>` correctly. */
export const SectionLabel: Story = {
  args: { size: '6', component: 'h3', children: 'Columns' },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // Small type, but still level 3 to a screen reader — the point of the split.
    await expect(
      canvas.getByRole('heading', { level: 3, name: 'Columns' }),
    ).toBeInTheDocument();
  },
};

/** `className` is appended after the variant classes, so it can override the weight or
 *  colour the scale sets. */
export const ClassNameOverride: Story = {
  args: { size: '4', className: 'font-normal text-fgDecorative' },
};
