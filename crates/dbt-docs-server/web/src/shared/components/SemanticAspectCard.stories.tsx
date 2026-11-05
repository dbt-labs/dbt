import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, within } from 'storybook/test';

import { SemanticAspectCard } from './SemanticAspectCard';

const meta: Meta<typeof SemanticAspectCard> = {
  component: SemanticAspectCard,
  args: {
    name: 'ordered_at',
    type: 'time',
    description: 'Timestamp the order was placed, in UTC.',
  },
  decorators: [(Story) => <ul className="max-w-xl space-y-2">{Story()}</ul>],
};

export default meta;
type Story = StoryObj<typeof SemanticAspectCard>;

/** The type badge is upper-cased, so it reads the same whether the caller passes a
 *  dimension type or a measure aggregation. */
export const Default: Story = {};

/** Used for measures, where `type` is the aggregation. */
export const Measure: Story = {
  args: {
    name: 'order_total',
    type: 'sum',
    description: 'Sum of `amount` across the order.',
  },
};

/** Descriptions are markdown here too — same plugin set as `DescriptionDisplay`. */
export const MarkdownDescription: Story = {
  args: {
    description:
      'Order **placement** time. See [the semantic layer docs](https://docs.getdbt.com).',
  },
};

/** Untrusted inline HTML that loads a frame or runs a script must not reach the DOM. */
export const ScriptPayload: Story = {
  args: {
    description: `<iframe srcdoc="&lt;script&gt;parent.document.documentElement.dataset.dbtPoc='executed'&lt;/script&gt;"></iframe>`,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvasElement.querySelector('iframe')).toBeNull();
    void canvas;
  },
};

export const WithoutDescription: Story = {
  args: { description: null },
};

/** A nameless aspect renders nothing — the guard that keeps a malformed semantic
 *  manifest from producing blank cards. */
export const NoNameRendersNothing: Story = {
  args: { name: null },
};
