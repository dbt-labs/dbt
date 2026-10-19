import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, waitFor, within } from 'storybook/test';

import { CodeSnippet } from './CodeSnippet';

/**
 * A syntax-highlighted code block. Highlighting is done by Shiki, which is imported
 * lazily on first use and themed with the app's `--fgSyntax*` CSS variables, so a
 * snippet follows the light/dark switcher without re-highlighting.
 *
 * Two behaviours worth knowing:
 *
 * - Highlighting is asynchronous. Until it resolves — and permanently, if `language` is
 *   omitted — the code renders in a plain `<pre>`. Callers use that on purpose for
 *   large files, where tokenizing costs more than the colour is worth (see
 *   `CodePreview`).
 * - `includeCopyButton` writes to `navigator.clipboard` and flips its icon to a
 *   checkmark for 1.5s. There is no permission fallback, so in a context without
 *   clipboard access the icon still flips while nothing is copied.
 */
const SQL = `with orders as (
    select * from {{ ref('stg_orders') }}
)

select
    customer_id,
    count(*) as number_of_orders,
    sum(amount) as lifetime_value
from orders
group by 1`;

const meta: Meta<typeof CodeSnippet> = {
  component: CodeSnippet,
  args: { code: SQL, language: 'sql' },
  decorators: [(Story) => <div className="w-[560px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof CodeSnippet>;

/** SQL, the overwhelmingly common case. */
export const Sql: Story = {
  play: async ({ canvasElement }) => {
    // The plain fallback is a bare <pre> with no element children; once Shiki resolves,
    // every token is a <span>. So this waits for real highlighting, not just a render.
    await waitFor(
      () => expect(canvasElement.querySelector('pre span')).not.toBeNull(),
      { timeout: 10_000 },
    );
  },
};

/** No `language`: the plain-text path. This is what the file preview falls back to for
 *  large files, and it is a deliberate choice rather than a failure state. */
export const NoLanguage: Story = {
  args: { language: undefined },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText(/number_of_orders/)).toBeVisible();
    // Unhighlighted: no token spans, ever.
    await expect(canvasElement.querySelector('pre span')).toBeNull();
  },
};

/** With the copy button, pinned top-right over the code. Not clicked here: the click
 *  writes to the real clipboard, which is not something a story should need permission
 *  for. */
export const WithCopyButton: Story = {
  args: { includeCopyButton: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole('button', { name: 'Copy code' })).toBeVisible();
  },
};

/** The dbt commands the empty states suggest — short, one-line, with a copy button, and
 *  the single most common use of this component outside the code tab. */
export const ShellCommand: Story = {
  args: {
    language: 'bash',
    code: 'dbt compile --write-index --static-analysis strict && dbt docs generate',
    includeCopyButton: true,
  },
};

/** YAML, for a schema file or a project config. */
export const Yaml: Story = {
  args: {
    language: 'yaml',
    code: `models:
  - name: customers
    description: One row per customer.
    columns:
      - name: customer_id
        tests:
          - unique
          - not_null`,
  },
};

/** JSON, for a manifest excerpt or an artifact payload. */
export const Json: Story = {
  args: {
    language: 'json',
    code: `{
  "unique_id": "model.jaffle_shop.customers",
  "resource_type": "model",
  "config": { "materialized": "table" },
  "depends_on": ["model.jaffle_shop.stg_customers"]
}`,
  },
};

/** Python, for a dbt Python model. */
export const Python: Story = {
  args: {
    language: 'python',
    code: `def model(dbt, session):
    dbt.config(materialized="table")
    orders = dbt.ref("stg_orders")
    return orders.groupBy("customer_id").count()`,
  },
};

/** A single long line: the block scrolls horizontally rather than wrapping, so
 *  indentation stays truthful. */
export const LongLine: Story = {
  args: {
    code:
      'select customer_id, first_order_date, most_recent_order_date, ' +
      'number_of_orders, lifetime_value, customer_lifetime_value_rank from customers',
  },
};

/** A tall snippet keeps its own height — the container scrolls the page, not the block,
 *  so a caller that needs a cap should pass one via `className`. */
export const Tall: Story = {
  args: {
    className: 'max-h-64 overflow-y-auto',
    code: Array.from(
      { length: 30 },
      (_, i) => `select * from stg_model_${i + 1} union all`,
    ).join('\n'),
  },
};
