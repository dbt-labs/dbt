import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';

import { useLineageStore } from '../../stores/lineageStore';
import { DagResourceFilterDropdown } from './DagResourceFilterDropdown';

const openMenu = () =>
  fireEvent.keyDown(screen.getByRole('button', { name: 'Filter' }), { key: 'Enter' });

/** `label=checked` for every checkbox in the menu, opened fresh and closed again. */
async function menuState(): Promise<string[]> {
  openMenu();
  const items = await screen.findAllByRole('menuitemcheckbox');
  const state = items.map(
    (item) => `${item.textContent}=${item.getAttribute('aria-checked')}`,
  );
  fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
  return state;
}

const visible = () => useLineageStore.getState().visibleResourceTypes;

beforeEach(() => {
  useLineageStore.getState().reset();
});

describe('DagResourceFilterDropdown', () => {
  it('adds a checked type to the ones still on after Deselect all', async () => {
    useLineageStore.getState().startHydration('source.jaffle_shop.raw.customers', 1, 1);
    render(<DagResourceFilterDropdown />);

    openMenu();
    fireEvent.click(await screen.findByText('Deselect all'));
    fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    // Models and the root's own type stay on -- in the store, not just on screen.
    expect(visible()).toEqual(new Set(['model', 'source']));

    openMenu();
    fireEvent.click(await screen.findByRole('menuitemcheckbox', { name: 'Exposures' }));
    expect(visible()).toEqual(new Set(['model', 'source', 'exposure']));
    expect(await menuState()).toEqual([
      'Models=true',
      'Sources=true',
      'Seeds=false',
      'Exposures=true',
    ]);
  });

  it("keeps the root's own type on and locked, even one that is off by default", async () => {
    useLineageStore.getState().startHydration('test.jaffle_shop.not_null_id', 1, 1);
    render(<DagResourceFilterDropdown />);
    expect(visible().has('test')).toBe(true);

    openMenu();
    fireEvent.click(await screen.findByText('See more'));
    const tests = await screen.findByRole('menuitemcheckbox', { name: 'Tests' });
    expect(tests.getAttribute('aria-checked')).toBe('true');
    expect(tests.hasAttribute('data-disabled')).toBe(true);
  });
});
