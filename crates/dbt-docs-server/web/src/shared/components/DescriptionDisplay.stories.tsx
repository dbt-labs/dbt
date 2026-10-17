import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, within } from 'storybook/test';

import { DescriptionDisplay } from './DescriptionDisplay';

const meta: Meta<typeof DescriptionDisplay> = {
  component: DescriptionDisplay,
  args: {
    description: 'One row per customer, with their first and most recent order dates.',
  },
};

export default meta;
type Story = StoryObj<typeof DescriptionDisplay>;

export const Default: Story = {};

/** Descriptions are markdown, and dbt users write real markdown in them —
 *  `remark-gfm` is loaded specifically for the table and strikethrough syntax. */
export const Markdown: Story = {
  args: {
    description: [
      '## Customers',
      '',
      'One row per customer. Grain is **one row per `customer_id`**.',
      '',
      '| Column | Meaning |',
      '| --- | --- |',
      '| `customer_id` | Surrogate key |',
      '| `number_of_orders` | Lifetime orders |',
      '',
      '- ~~Deprecated~~ as of 2026-01',
      '- See [the runbook](https://example.com/runbook)',
      '',
      '```sql',
      'select count(*) from analytics.dbt.customers',
      '```',
    ].join('\n'),
  },
};

/** `rehype-sanitize` keeps formatting tags in a description and drops script, frames, and event handlers. */
export const InlineHtml: Story = {
  args: {
    description:
      'Owned by <b>finance analytics</b>.<br/>Contact <i>finance@example.com</i>.',
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const bold = canvas.getByText('finance analytics', { selector: 'b' });
    await expect(bold).toBeInTheDocument();
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

/** External links are rewritten to `target="_blank"` by `rehype-external-links`. */
export const ExternalLink: Story = {
  args: { description: 'Defined upstream in [Fivetran](https://fivetran.com).' },
};

/** The undocumented case: italic placeholder copy, not an empty block. This is the
 *  most common state in a real project. */
export const NoDescription: Story = {
  args: { description: null },
};

/** Empty string takes the same branch as null — it is falsy, and an empty
 *  `description:` key in a `.yml` should read as undocumented rather than as a
 *  description that happens to be blank. */
export const EmptyString: Story = {
  args: { description: '' },
};
