import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import { useQueryHistory } from './useQueryHistory';

const STORAGE_KEY = 'dbt-docs-v2:query-history';

describe('useQueryHistory', () => {
  afterEach(() => sessionStorage.clear());

  it('starts empty when nothing is stored', () => {
    const { result } = renderHook(() => useQueryHistory());
    expect(result.current.entries).toEqual([]);
  });

  it('adds a pushed entry to the front', () => {
    const { result } = renderHook(() => useQueryHistory());
    act(() => result.current.push('select 1'));
    expect(result.current.entries).toEqual(['select 1']);
  });

  it('does not add a consecutive duplicate', () => {
    const { result } = renderHook(() => useQueryHistory());
    act(() => result.current.push('select 1'));
    act(() => result.current.push('select 1'));
    expect(result.current.entries).toEqual(['select 1']);
  });

  it('moves a non-consecutive duplicate to the front and removes the older instance', () => {
    const { result } = renderHook(() => useQueryHistory());
    act(() => result.current.push('select 1'));
    act(() => result.current.push('select 2'));
    act(() => result.current.push('select 1'));
    expect(result.current.entries).toEqual(['select 1', 'select 2']);
  });

  it('is a no-op when pushing an empty or whitespace-only string', () => {
    const { result } = renderHook(() => useQueryHistory());
    act(() => result.current.push('   '));
    expect(result.current.entries).toEqual([]);
  });

  it('trims values before storing', () => {
    const { result } = renderHook(() => useQueryHistory());
    act(() => result.current.push('  select 1  '));
    expect(result.current.entries).toEqual(['select 1']);
  });

  it('caps history at MAX_ENTRIES, dropping the oldest', () => {
    const { result } = renderHook(() => useQueryHistory());
    for (let i = 1; i <= 21; i += 1) {
      act(() => result.current.push(`select ${i}`));
    }
    expect(result.current.entries).toHaveLength(20);
    expect(result.current.entries[0]).toBe('select 21');
    expect(result.current.entries).not.toContain('select 1');
  });

  it('persists entries to sessionStorage', async () => {
    const { result } = renderHook(() => useQueryHistory());
    act(() => result.current.push('select 1'));
    await waitFor(() => {
      const raw = sessionStorage.getItem(STORAGE_KEY);
      expect(raw).not.toBeNull();
      expect(JSON.parse(raw as string)).toEqual(['select 1']);
    });
  });
});
