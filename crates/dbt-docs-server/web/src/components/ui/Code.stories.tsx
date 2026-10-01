import type { Meta, StoryObj } from '@storybook/react-vite';

import { Code } from './Code';

/**
 * Inline `<code>` for a single identifier, path or flag inside running text. For a
 * whole block — anything multi-line, or anything wanting syntax highlighting or a copy
 * button — use `CodeSnippet` instead.
 */
const meta: Meta<typeof Code> = {
  component: Code,
  args: { children: 'models/marts/customers.sql' },
};

export default meta;
type Story = StoryObj<typeof Code>;

export const Default: Story = {};

/** The real use: mid-sentence, where the shaded background separates the literal from
 *  the prose around it. `inline-flex` is deliberately not used, so it stays on the
 *  text baseline and wraps with the paragraph. */
export const InProse: Story = {
  render: () => (
    <p className="max-w-md text-sm text-fgMain">
      Nothing was written to <Code>target/</Code> because the run ended before compile.
      Re-run with <Code>--write-index</Code> to populate the docs site.
    </p>
  ),
};

/** A unique_id is the longest thing that realistically lands here. There is no
 *  truncation and no break-anywhere, so a long literal will push its container. */
export const LongIdentifier: Story = {
  args: { children: 'model.jaffle_shop.int_order_items_joined_to_customers' },
  decorators: [(Story) => <div className="w-[280px]">{Story()}</div>],
};

/** Error messages pair it with a label — the pattern the lineage and detail views use
 *  for a failed fetch. */
export const InErrorMessage: Story = {
  render: () => (
    <div className="err">
      Failed to load lineage: <Code className="inline">no such table: lineage</Code>
    </div>
  ),
};
