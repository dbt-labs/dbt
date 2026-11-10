import type { Meta, StoryObj } from '@storybook/react-vite';

import { Markdown } from './Markdown';

const meta: Meta<typeof Markdown> = {
  component: Markdown,
  args: { children: '# Jaffle Shop\n\nThe canonical example project.' },
  decorators: [(Story) => <div className="max-w-3xl space-y-4">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof Markdown>;

export const Default: Story = {};

/**
 * Every heading level. This is the load-bearing story: tailwind preflight is on and
 * there is no typography plugin, so any element missing from the component map renders
 * at body size and weight. dbt Core's own default overview is written entirely in
 * `###`/`####`, so h3/h4 in particular are what an unconfigured project's landing page
 * depends on.
 */
export const AllHeadingLevels: Story = {
  args: {
    children: [
      '# Heading 1',
      '## Heading 2',
      '### Heading 3',
      '#### Heading 4',
      '##### Heading 5',
      '###### Heading 6',
      '',
      'Body text for scale comparison.',
    ].join('\n'),
  },
};

/**
 * Lists are the other preflight casualty: `list-disc` / `list-decimal` are explicit
 * because preflight strips the markers. If the markers are missing here, the classes
 * were dropped — or a `flex` crept into the container, which suppresses them too.
 */
export const Lists: Story = {
  args: {
    children: [
      '- First bullet',
      '- Second bullet',
      '  - Nested bullet',
      '',
      '1. First numbered',
      '2. Second numbered',
      '   1. Nested numbered',
    ].join('\n'),
  },
};

/** A fenced block renders as `pre > code`; the inner element must not repeat the
 *  background. Inline code should keep its own. */
export const Code: Story = {
  args: {
    children: [
      'Run `dbt docs generate` first.',
      '',
      '```sql',
      'select customer_id, count(*)',
      'from analytics.dbt.orders',
      'group by 1',
      '```',
    ].join('\n'),
  },
};

/** GFM tables — the reason `remark-gfm` is loaded. */
export const Table: Story = {
  args: {
    children: [
      '| Model | Grain | Owner |',
      '| --- | --- | --- |',
      '| `customers` | one row per customer | finance |',
      '| `orders` | one row per order | finance |',
    ].join('\n'),
  },
};

export const Blockquote: Story = {
  args: {
    children: '> Regenerate the docs after every production run.\n\nBody text follows.',
  },
};

/** External links get `target="_blank"` and `rel="noreferrer"` from
 *  `rehype-external-links`. */
export const Links: Story = {
  args: {
    children: 'See the [dbt documentation](https://docs.getdbt.com) for more.',
  },
};

/**
 * `rehypeRaw` is on, so authored HTML renders as HTML. This is a deliberate
 * divergence from docs v1, which escaped it — and it is why HTML comments in the
 * bundled default overview are invisible rather than printed.
 */
export const RawHtml: Story = {
  args: {
    children: [
      '<!-- this comment should not be visible -->',
      '',
      'Owned by <b>finance analytics</b>.',
      '',
      '<div style="padding: 8px; border: 1px solid currentColor">An authored div.</div>',
    ].join('\n'),
  },
};

export const HorizontalRule: Story = {
  args: { children: 'Above.\n\n---\n\nBelow.' },
};

/** An image with no alt text is marked decorative rather than announced by filename;
 *  an authored alt still wins. */
export const Images: Story = {
  args: {
    children:
      '![](https://placehold.co/240x80/png)\n\n![Project diagram](https://placehold.co/240x80/png)',
  },
};

/** A whitespace-only overview renders nothing, so the landing page can fall back to
 *  its own default rather than showing an empty block. */
export const BlankRendersNothing: Story = {
  args: { children: '   \n\n  ' },
};

/** Everything at once — closest to a real, thoroughly-authored overview. */
export const FullDocument: Story = {
  args: {
    children: [
      '# Jaffle Shop',
      '',
      'The canonical example project, documented end to end.',
      '',
      '## Getting started',
      '',
      'Run `dbt build` then `dbt docs generate`.',
      '',
      '### Marts',
      '',
      '- `customers` — one row per customer',
      '- `orders` — one row per order',
      '',
      '#### Conventions',
      '',
      '1. Staging models are views',
      '2. Marts are tables',
      '',
      '> Sources are refreshed hourly by the loader.',
      '',
      '| Layer | Materialization |',
      '| --- | --- |',
      '| staging | view |',
      '| marts | table |',
      '',
      '```sql',
      "select * from {{ ref('customers') }}",
      '```',
      '',
      'Questions? See [the runbook](https://example.com/runbook).',
    ].join('\n'),
  },
};
