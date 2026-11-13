import type { Meta, StoryObj } from '@storybook/react-vite';

import { HealthIssueType } from '../../typings/discoveryEnums';
import { SearchResultItem } from './SearchResultItem';
import type { SearchResultDisplayData } from './types';

function hit(
  overrides: Partial<SearchResultDisplayData['hit']> = {},
): SearchResultDisplayData['hit'] {
  return {
    name: 'customers',
    uniqueId: 'model.jaffle_shop.customers',
    resourceType: 'model',
    ...overrides,
  };
}

const meta: Meta<typeof SearchResultItem> = {
  component: SearchResultItem,
  args: {
    query: 'customer',
    data: { matchedField: 'name', highlight: '<b>customer</b>s', hit: hit() },
    getResourceHref: (uniqueId) => `#/model/${uniqueId}`,
  },
  decorators: [(Story) => <ul className="w-[720px] space-y-2">{Story()}</ul>],
};

export default meta;
type Story = StoryObj<typeof SearchResultItem>;

/** A name match. Note there is no second line: the name is already bolded in the
 *  link, so repeating it would be redundant. */
export const NameMatch: Story = {};

/**
 * For models and sources the label is the uniqueId with its
 * `{resourceType}.{package}.` prefix stripped — so a versioned model reads
 * `customers.v2` rather than losing the suffix.
 */
export const VersionedModelName: Story = {
  args: {
    data: {
      matchedField: 'name',
      highlight: null,
      hit: hit({ uniqueId: 'model.jaffle_shop.customers.v2' }),
    },
  },
};

/** A description match adds a second line, framed with ellipses on both sides to
 *  signal it is an excerpt. */
export const DescriptionMatch: Story = {
  args: {
    data: {
      matchedField: 'description',
      highlight: 'one row per <b>customer</b>, with order counts',
      hit: hit(),
    },
  },
};

/** Column matches list up to five columns, upper-cased, with an ellipsis when there
 *  are more. */
export const ColumnMatch: Story = {
  args: {
    data: {
      matchedField: 'column',
      highlight: '<b>customer</b>_id, <b>customer</b>_name',
      hit: hit(),
    },
  },
};

export const ColumnMatchTruncatedToFive: Story = {
  args: {
    data: {
      matchedField: 'column',
      highlight: Array.from({ length: 9 }, (_, i) => `<b>customer</b>_field_${i}`).join(
        ', ',
      ),
      hit: hit(),
    },
  },
};

/** With `getColumnHref` the matched columns become deep links into the column list. */
export const ColumnMatchWithLinks: Story = {
  args: {
    data: {
      matchedField: 'column',
      highlight: '<b>customer</b>_id, <b>customer</b>_name',
      hit: hit(),
    },
    getColumnHref: (uniqueId, column) => `#/model/${uniqueId}?column=${column}`,
  },
};

export const TagMatch: Story = {
  args: {
    data: {
      matchedField: 'tag',
      highlight: '<b>customer</b>_facing',
      hit: hit(),
    },
  },
};

/** The "View lineage" CTA needs both a builder *and* an `fqn` on the hit — either
 *  alone renders nothing. */
export const WithLineageLink: Story = {
  args: {
    data: {
      matchedField: 'name',
      highlight: null,
      hit: hit({ fqn: ['jaffle_shop', 'marts', 'customers'] }),
    },
    getLineageHref: (uniqueId) => `#/lineage?select=${uniqueId}`,
  },
};

export const FqnWithoutLineageBuilder: Story = {
  args: {
    data: {
      matchedField: 'name',
      highlight: null,
      hit: hit({ fqn: ['jaffle_shop', 'marts', 'customers'] }),
    },
  },
};

/** Trust signals are opt-in per row. docs-v2 has no health data, so it omits them
 *  entirely and no badge appears. */
export const WithTrustSignals: Story = {
  args: {
    trustSignals: { healthIssues: [HealthIssueType.NoTests] },
  },
};

/** Non-model/source types show their `name` rather than the stripped uniqueId. */
const OTHER_TYPE_RESULTS: SearchResultDisplayData[] = (
  ['test', 'metric', 'exposure', 'macro', 'seed'] as const
).map((type) => ({
  matchedField: 'name',
  highlight: null,
  hit: hit({
    name: `${type}_thing`,
    uniqueId: `${type}.jaffle_shop.${type}_thing`,
    resourceType: type,
  }),
}));

export const OtherResourceTypes: Story = {
  render: () => (
    <>
      {OTHER_TYPE_RESULTS.map((data) => (
        <SearchResultItem
          key={data.hit.uniqueId}
          query="thing"
          data={data}
          getResourceHref={(uniqueId) => `#/resource/${uniqueId}`}
        />
      ))}
    </>
  ),
};

/** The loading placeholder, sized to match a real row so the list does not jump. */
export const Skeleton: Story = {
  render: () => <SearchResultItem skeleton />,
};

/** A hit with no name renders nothing rather than an empty row. */
export const NamelessHitRendersNothing: Story = {
  args: {
    data: { matchedField: 'name', highlight: null, hit: hit({ name: null }) },
  },
};
