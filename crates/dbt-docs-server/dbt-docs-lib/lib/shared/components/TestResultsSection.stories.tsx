import type { Meta, StoryObj } from '@storybook/react-vite';

import { type SharedTestResult, TestResultsSection } from './TestResultsSection';

function test(name: string, status: string | null): SharedTestResult {
  return { name, uniqueId: `test.jaffle_shop.${name}`, status };
}

const details = (
  <div className="border-t border-borderMuted p-4 text-sm text-fgAlt">
    Per-test rows go here.
  </div>
);

const meta: Meta<typeof TestResultsSection> = {
  component: TestResultsSection,
  args: {
    resourceType: 'model',
    tests: [test('not_null_customers_customer_id', 'pass')],
    toExpand: details,
  },
  decorators: [
    (Story) => (
      <div className="w-[560px] rounded-md border border-borderMuted">{Story()}</div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof TestResultsSection>;

/** All passing: the success icon, and the tooltip says so. */
export const AllPassing: Story = {};

/** Any failure wins over any warning — the roll-up is worst-status, not a tally. */
export const OneFailing: Story = {
  args: {
    tests: [
      test('not_null_customers_customer_id', 'pass'),
      test('unique_customers_customer_id', 'fail'),
      test('accepted_values_orders_status', 'warn'),
    ],
  },
};

export const OneWarning: Story = {
  args: {
    tests: [
      test('not_null_customers_customer_id', 'pass'),
      test('accepted_values_orders_status', 'warn'),
    ],
  },
};

/** An empty list is a *warning*, not a success: "no tests configured" is a real
 *  finding, and rendering it as passing would be misleading. */
export const NoTestsConfigured: Story = {
  args: { tests: [] },
};

/** `undefined` means "we could not determine this" and renders nothing at all —
 *  distinct from the empty array above. */
export const UnknownRendersNothing: Story = {
  args: { tests: undefined },
};

/** Sorted worst-first, then alphabetically within a status. Expand to check the order
 *  against `testStatusOrder`. */
export const SortedBySeverity: Story = {
  args: {
    tests: [
      test('zzz_passing_test', 'pass'),
      test('aaa_skipped_test', 'skipped'),
      test('mmm_warning_test', 'warn'),
      test('bbb_error_test', 'error'),
      test('aaa_failing_test', 'fail'),
      test('ccc_unrecognised_status', 'something_new'),
    ],
  },
};

/** Without `toExpand` the row is not expandable — there is nothing to reveal. */
export const NotExpandable: Story = {
  args: { toExpand: undefined },
};

/** `resourceType` only appears in the tooltip copy, which is why it is a plain
 *  string. */
export const SourceResourceType: Story = {
  args: { resourceType: 'source' },
};
