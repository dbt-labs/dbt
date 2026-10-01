import type { Meta, StoryObj } from '@storybook/react-vite';

import { BoldedText, BoldSearchHighlight, SanitizeBoldText } from './BoldedText';

const meta: Meta<typeof BoldedText> = {
  component: BoldedText,
  args: { text: 'stg_customers', shouldBeBold: 'customer' },
};

export default meta;
type Story = StoryObj<typeof BoldedText>;

/** `BoldedText` bolds client-side, by matching the query against the text. */
export const Default: Story = {};

/** Every occurrence is bolded, not just the first. */
export const MultipleMatches: Story = {
  args: { text: 'customers_to_customer_orders', shouldBeBold: 'customer' },
};

/** Matching is case-insensitive but the original casing is preserved in the output. */
export const CaseInsensitive: Story = {
  args: { text: 'STG_Customers', shouldBeBold: 'customers' },
};

/**
 * Regex metacharacters in the query are escaped, so a search for `order.total` or
 * `a+b` cannot blow up or match wildly. Dots and spaces are additionally treated as
 * alternation, so this query bolds both words.
 */
export const QueryWithMetacharacters: Story = {
  args: { text: 'analytics.dbt.customers (v2+)', shouldBeBold: 'dbt.customers' },
};

/** An empty query short-circuits to plain truncated text. */
export const EmptyQuery: Story = {
  args: { shouldBeBold: '' },
};

export const NoMatch: Story = {
  args: { text: 'orders', shouldBeBold: 'customer' },
};

export const CustomBoldStyling: Story = {
  args: { boldProps: { className: 'bg-bgBrandMuted' } },
};

/**
 * `SanitizeBoldText` takes the other route: the *backend* has already marked the
 * matches with `<b>…</b>`, and this turns those tags into elements rather than
 * rendering them as text. It never injects HTML — the fragment is split on the tags.
 */
export const BackendSuppliedHighlight: Story = {
  render: () => (
    <SanitizeBoldText text="stg_<b>customer</b>s_joined" query="customer" />
  ),
};

/** Any other markup in the fragment stays inert text, which is the point of the
 *  "sanitize" in the name. */
export const BackendHighlightWithOtherMarkup: Story = {
  render: () => (
    <SanitizeBoldText
      text="<b>customer</b>s <script>alert(1)</script>"
      query="customer"
    />
  ),
};

/**
 * `BoldSearchHighlight` combines both: it strips the backend's tags, and if the raw
 * query appears in the result it re-marks that instead — so a hit whose highlight
 * missed the user's exact string still shows it bolded.
 */
export const CombinedHighlight: Story = {
  render: () => (
    <div className="space-y-1">
      {/* Query present after stripping: re-marked around the query. */}
      <BoldSearchHighlight text="stg_<b>cust</b>omers" query="customers" />
      {/* Query absent: the backend's own marks are kept. */}
      <BoldSearchHighlight text="stg_<b>cust</b>omers" query="zzz" />
    </div>
  ),
};

/** Null text renders nothing. */
export const NullTextRendersNothing: Story = {
  render: () => <BoldSearchHighlight text={null} query="customer" />,
};
