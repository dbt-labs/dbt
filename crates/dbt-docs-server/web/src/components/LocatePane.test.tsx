import type { ComponentProps } from 'react';
import { fireEvent, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { AssetFilters } from '../App';
import type {
  AssetCounts,
  FileEntry,
  Project,
  SearchFacets,
  UserState,
} from '../shared';
import { renderWithProviders } from '../test/renderWithProviders';
import type { NodeSummary } from '../types';
import { LocatePane } from './LocatePane';

const PROJECT: Project = { name: 'jaffle_shop', gitBranch: 'main', gitIsDirty: true };

const NODES: NodeSummary[] = [
  {
    unique_id: 'model.jaffle_shop.orders',
    name: 'orders',
    resource_type: 'model',
    package_name: 'jaffle_shop',
    materialized: 'table',
    original_file_path: 'models/marts/orders.sql',
  },
  {
    unique_id: 'source.jaffle_shop.raw.orders',
    name: 'orders',
    resource_type: 'source',
    package_name: 'jaffle_shop',
  },
];

const NODES_MULTI_PKG: NodeSummary[] = [
  ...NODES,
  {
    unique_id: 'model.dbt_utils.helper',
    name: 'helper',
    resource_type: 'model',
    package_name: 'dbt_utils',
  },
];

function makeFilters(overrides: Partial<AssetFilters> = {}): AssetFilters {
  return {
    resourceType: [],
    modelingLayer: [],
    materialization: [],
    pkg: [],
    tag: [],
    ...overrides,
  };
}

function makeProps(
  overrides: Partial<ComponentProps<typeof LocatePane>> = {},
): ComponentProps<typeof LocatePane> {
  return {
    project: PROJECT,
    nodes: NODES,
    files: [],
    selectedId: null,
    previewId: null,
    isListView: false,
    onPeek: vi.fn(),
    onSelect: vi.fn(),
    onShowList: vi.fn(),
    onShowProject: vi.fn(),
    isHome: true,
    query: '',
    theme: 'dark',
    onSetTheme: vi.fn(),
    filters: makeFilters(),
    onSetFilters: vi.fn(),
    mode: 'assets',
    onSelectMode: vi.fn(),
    searchFacets: null,
    assetCounts: null,
    userState: null,
    ...overrides,
  };
}

/** Find a checkbox row rendered by `CheckboxRow` (`<label>` wraps the input
 *  plus a label span and an optional count span). Locating by the label
 *  span's exact text and scoping into that `<label>` avoids relying on the
 *  accessible-name computation, which changes when the row is disabled
 *  (its wrapping `<label>` gets an `aria-label` override). */
function checkboxRow(label: string): HTMLElement {
  const row = screen.getByText(label, { selector: 'span' }).closest('label');
  if (!row) throw new Error(`checkbox row not found for label "${label}"`);
  return within(row).getByRole('checkbox');
}

describe('<LocatePane /> — assets mode', () => {
  it('renders the project root and one row per RESOURCE_TYPE_ORDER type using assetCounts as authoritative', () => {
    const assetCounts: AssetCounts = { model: 4242, source: 3 };
    renderWithProviders(
      <LocatePane {...makeProps({ assetCounts, nodes: [NODES[0]] })} />,
    );

    // Project root row.
    expect(screen.getByText('jaffle_shop')).toBeInTheDocument();

    // Every non-analysis RESOURCE_TYPE_ORDER type renders a row.
    expect(screen.getByText('Models')).toBeInTheDocument();
    expect(screen.getByText('Sources')).toBeInTheDocument();
    expect(screen.getByText('Tests')).toBeInTheDocument();
    expect(screen.getByText('Exposures')).toBeInTheDocument();
    expect(screen.getByText('Groups')).toBeInTheDocument();
    expect(screen.getByText('Metrics')).toBeInTheDocument();
    expect(screen.getByText('Semantic models')).toBeInTheDocument();
    expect(screen.getByText('Seeds')).toBeInTheDocument();
    expect(screen.getByText('Macros')).toBeInTheDocument();
    expect(screen.getByText('Snapshots')).toBeInTheDocument();
    expect(screen.getByText('Saved queries')).toBeInTheDocument();
    // FEATURE_FLAGS.hasAnalysis is false — no Analyses row.
    expect(screen.queryByText('Analyses')).not.toBeInTheDocument();

    // Count comes from assetCounts (4242), not the single node in `nodes`.
    // The component uses locale-aware `toLocaleString()`, so build the expected
    // grouped string the same way rather than hardcoding en-US's '4,242'.
    expect(screen.getByText((4242).toLocaleString())).toBeInTheDocument();
    expect(screen.queryByText('1')).not.toBeInTheDocument();
  });

  it('clicking a type row calls onShowList(type); clicking the project root calls onShowProject()', () => {
    const onShowList = vi.fn();
    const onShowProject = vi.fn();
    renderWithProviders(<LocatePane {...makeProps({ onShowList, onShowProject })} />);

    fireEvent.click(screen.getByText('Models'));
    expect(onShowList).toHaveBeenCalledWith('model');

    fireEvent.click(screen.getByText('jaffle_shop'));
    expect(onShowProject).toHaveBeenCalled();
  });

  it('shows the multi-package hint only when nodes span more than one package', () => {
    const { rerender } = renderWithProviders(
      <LocatePane {...makeProps({ nodes: NODES })} />,
    );
    expect(screen.queryByText(/packages installed/)).not.toBeInTheDocument();

    rerender(<LocatePane {...makeProps({ nodes: NODES_MULTI_PKG })} />);
    expect(screen.getByText(/2 packages installed/)).toBeInTheDocument();
  });
});

describe('<LocatePane /> — files mode', () => {
  const FILES: FileEntry[] = [
    {
      uniqueId: 'model.jaffle_shop.stg_orders',
      name: 'stg_orders',
      resourceType: 'model',
      packageName: 'pkg_a',
      originalFilePath: 'stg_orders.sql',
    },
    {
      uniqueId: 'analysis.jaffle_shop.my_analysis',
      name: 'my_analysis',
      resourceType: 'analysis',
      packageName: 'pkg_a',
      originalFilePath: 'my_analysis.sql',
    },
  ];

  it('renders without crashing and shows the project root', () => {
    renderWithProviders(<LocatePane {...makeProps({ mode: 'files', files: FILES })} />);
    expect(screen.getByText('jaffle_shop')).toBeInTheDocument();
    expect(screen.getByText('pkg_a')).toBeInTheDocument();
  });

  it('excludes analysis-typed files from the tree while FEATURE_FLAGS.hasAnalysis is false', async () => {
    renderWithProviders(<LocatePane {...makeProps({ mode: 'files', files: FILES })} />);

    // Expand the package folder to reveal its file children.
    fireEvent.click(screen.getByText('pkg_a'));

    expect(await screen.findByText('stg_orders.sql')).toBeInTheDocument();
    expect(screen.queryByText('my_analysis.sql')).not.toBeInTheDocument();
  });
});

describe('<LocatePane /> — filter mode', () => {
  const SEARCH_FACETS: SearchFacets = {
    accesses: [],
    modelingLayers: [
      { value: 'Staging', count: 2 },
      { value: 'Marts', count: 3 },
    ],
    materializationTypes: [
      { value: 'table', count: 3 },
      { value: 'view', count: 0 },
    ],
    tags: [
      { value: 'finance', count: 2 },
      { value: 'pii', count: 1 },
    ],
    packages: [
      { value: 'jaffle_shop', count: 5 },
      { value: 'pkg_a', count: 1 },
      { value: 'pkg_b', count: 1 },
      { value: 'pkg_c', count: 1 },
      { value: 'pkg_d', count: 1 },
      { value: 'pkg_e', count: 1 },
      { value: 'pkg_f', count: 1 },
      { value: 'pkg_g', count: 1 },
      { value: 'pkg_h', count: 1 },
      { value: 'pkg_i', count: 1 },
    ],
  };

  it('renders FilterSections for Asset type, Modeling layer, Materialization, and Package, but not Tag when searchFacets is null', () => {
    renderWithProviders(
      <LocatePane {...makeProps({ mode: 'filter', searchFacets: null })} />,
    );
    expect(screen.getByRole('heading', { name: 'Asset type' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Modeling layer' })).toBeInTheDocument();
    expect(
      screen.getByRole('heading', { name: 'Materialization' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Package' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Tag' })).not.toBeInTheDocument();
  });

  it('renders the Tag section only when searchFacets.tags is non-empty', () => {
    renderWithProviders(
      <LocatePane {...makeProps({ mode: 'filter', searchFacets: SEARCH_FACETS })} />,
    );
    expect(screen.getByRole('heading', { name: 'Tag' })).toBeInTheDocument();
  });

  it('toggling an unchecked Asset type checkbox adds the value via onSetFilters', () => {
    const onSetFilters = vi.fn();
    renderWithProviders(
      <LocatePane
        {...makeProps({
          mode: 'filter',
          filters: makeFilters(),
          onSetFilters,
        })}
      />,
    );
    fireEvent.click(checkboxRow('Models'));
    expect(onSetFilters).toHaveBeenCalledWith(
      expect.objectContaining({ resourceType: ['model'] }),
    );
  });

  it('toggling a checked Asset type checkbox removes the value via onSetFilters', () => {
    const onSetFilters = vi.fn();
    renderWithProviders(
      <LocatePane
        {...makeProps({
          mode: 'filter',
          filters: makeFilters({ resourceType: ['model'] }),
          onSetFilters,
        })}
      />,
    );
    fireEvent.click(checkboxRow('Models'));
    expect(onSetFilters).toHaveBeenCalledWith(
      expect.objectContaining({ resourceType: [] }),
    );
  });

  it('"Select all" on Asset type selects only the non-zero-count options', () => {
    const onSetFilters = vi.fn();
    renderWithProviders(
      <LocatePane
        {...makeProps({
          mode: 'filter',
          nodes: NODES, // only 'model' and 'source' have non-zero counts
          onSetFilters,
        })}
      />,
    );
    const section = screen
      .getByRole('heading', { name: 'Asset type' })
      .closest('section');
    if (!section) throw new Error('Asset type section not found');
    fireEvent.click(within(section).getByText('Select all'));
    expect(onSetFilters).toHaveBeenCalledWith(
      expect.objectContaining({ resourceType: ['model', 'source'] }),
    );
  });

  it('"Clear all" on Modeling layer sets that key to []', () => {
    const onSetFilters = vi.fn();
    renderWithProviders(
      <LocatePane
        {...makeProps({
          mode: 'filter',
          searchFacets: SEARCH_FACETS,
          filters: makeFilters({ modelingLayer: ['Staging'] }),
          onSetFilters,
        })}
      />,
    );
    const section = screen
      .getByRole('heading', { name: 'Modeling layer' })
      .closest('section');
    if (!section) throw new Error('Modeling layer section not found');
    fireEvent.click(within(section).getByText('Clear all'));
    expect(onSetFilters).toHaveBeenCalledWith(
      expect.objectContaining({ modelingLayer: [] }),
    );
  });

  it('disables a checkbox only when its count is 0 and it is not already selected', () => {
    renderWithProviders(
      <LocatePane
        {...makeProps({
          mode: 'filter',
          searchFacets: SEARCH_FACETS,
        })}
      />,
    );
    const viewCheckbox = checkboxRow('View');
    expect(viewCheckbox).toBeDisabled();
    expect(viewCheckbox.closest('label')).toHaveAttribute(
      'aria-label',
      'No resources in this project match this filter',
    );

    const tableCheckbox = checkboxRow('Table');
    expect(tableCheckbox).not.toBeDisabled();
    expect(tableCheckbox.closest('label')).not.toHaveAttribute('aria-label');
  });

  it('a zero-count checkbox that is already selected is not disabled', () => {
    renderWithProviders(
      <LocatePane
        {...makeProps({
          mode: 'filter',
          searchFacets: SEARCH_FACETS,
          filters: makeFilters({ materialization: ['view'] }),
        })}
      />,
    );
    expect(checkboxRow('View')).not.toBeDisabled();
  });

  describe('Package section collapse/expand', () => {
    it('shows only the first 8 packages plus any hidden-but-selected ones, with a More(n) toggle', () => {
      renderWithProviders(
        <LocatePane
          {...makeProps({
            mode: 'filter',
            searchFacets: SEARCH_FACETS,
            filters: makeFilters({ pkg: ['pkg_i'] }),
          })}
        />,
      );
      const section = screen
        .getByRole('heading', { name: 'Package' })
        .closest('section');
      if (!section) throw new Error('Package section not found');
      const scoped = within(section);

      // Head (first 8, sorted with project name first then alpha).
      expect(scoped.getByText('jaffle_shop')).toBeInTheDocument();
      expect(scoped.getByText('pkg_a')).toBeInTheDocument();
      expect(scoped.getByText('pkg_g')).toBeInTheDocument();
      // Hidden-but-selected package still shows even while collapsed.
      expect(scoped.getByText('pkg_i')).toBeInTheDocument();
      // Not selected and past the default cutoff — hidden.
      expect(scoped.queryByText('pkg_h')).not.toBeInTheDocument();

      const moreButton = scoped.getByRole('button', { name: 'More (2)' });
      expect(moreButton).toHaveAttribute('aria-expanded', 'false');

      fireEvent.click(moreButton);
      expect(scoped.getByText('pkg_h')).toBeInTheDocument();
      const lessButton = scoped.getByRole('button', { name: 'Less' });
      expect(lessButton).toHaveAttribute('aria-expanded', 'true');

      fireEvent.click(lessButton);
      expect(scoped.queryByText('pkg_h')).not.toBeInTheDocument();
      expect(scoped.getByRole('button', { name: 'More (2)' })).toBeInTheDocument();
    });
  });
});

describe('<LocatePane /> — shared chrome', () => {
  it('clicking the Files/Filter tabs calls onSelectMode with the right value', () => {
    const onSelectMode = vi.fn();
    renderWithProviders(<LocatePane {...makeProps({ onSelectMode })} />);
    fireEvent.click(screen.getByRole('radio', { name: 'Files' }));
    expect(onSelectMode).toHaveBeenCalledWith('files');
    fireEvent.click(screen.getByRole('radio', { name: 'Filter' }));
    expect(onSelectMode).toHaveBeenCalledWith('filter');
  });

  // Two independent tests rather than two clicks on one render: the toggle
  // is a controlled radio group, so clicking an already-active segment is
  // correctly a no-op (nothing changed) — a real re-render after a theme
  // change would update the controlled value between clicks, which a single
  // static render can't simulate.
  it('clicking an inactive theme segment calls onSetTheme', () => {
    const onSetTheme = vi.fn();
    renderWithProviders(<LocatePane {...makeProps({ theme: 'dark', onSetTheme })} />);
    fireEvent.click(screen.getByRole('radio', { name: 'Light' }));
    expect(onSetTheme).toHaveBeenCalledWith('light');
  });

  it('clicking a different inactive theme segment also calls onSetTheme', () => {
    const onSetTheme = vi.fn();
    renderWithProviders(<LocatePane {...makeProps({ theme: 'light', onSetTheme })} />);
    fireEvent.click(screen.getByRole('radio', { name: 'Dark' }));
    expect(onSetTheme).toHaveBeenCalledWith('dark');
  });

  // TODO: verify that git branch is no longer displayed
  // it('renders the git branch and dirty dot when present', () => {
  //   renderWithProviders(
  //     <LocatePane
  //       {...makeProps({
  //         project: { name: 'jaffle_shop', gitBranch: 'feature/x', gitIsDirty: true },
  //       })}
  //     />,
  //   );
  //   expect(screen.getByText('feature/x')).toBeInTheDocument();
  //   expect(screen.getByLabelText('uncommitted changes')).toBeInTheDocument();
  // });

  it('omits the branch (and dirty dot) when no gitBranch is present', () => {
    renderWithProviders(
      <LocatePane {...makeProps({ project: { name: 'jaffle_shop' } })} />,
    );
    expect(screen.queryByTitle('Git branch')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('uncommitted changes')).not.toBeInTheDocument();
  });

  it('renders the UpgradeRailStack when userState is non-null and not when it is null', () => {
    const { container, rerender } = renderWithProviders(
      <LocatePane {...makeProps({ userState: null })} />,
    );
    expect(
      container.querySelector('.locate-pane__connect-hook'),
    ).not.toBeInTheDocument();

    rerender(<LocatePane {...makeProps({ userState: 'core' as UserState })} />);
    expect(container.querySelector('.locate-pane__connect-hook')).toBeInTheDocument();
  });
});
