import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import { useResizable } from './useResizable';

describe('useResizable', () => {
  afterEach(() => localStorage.clear());

  it('starts at defaultWidth when nothing is stored', () => {
    const { result } = renderHook(() => useResizable('panel-a', 200, 100, 400));
    expect(result.current.width).toBe(200);
  });

  it('initializes from a valid stored value within [min, max]', () => {
    localStorage.setItem('panel-b', '250');
    const { result } = renderHook(() => useResizable('panel-b', 200, 100, 400));
    expect(result.current.width).toBe(250);
  });

  it('falls back to defaultWidth when the stored value is outside [min, max]', () => {
    localStorage.setItem('panel-c', '9999');
    const { result } = renderHook(() => useResizable('panel-c', 200, 100, 400));
    expect(result.current.width).toBe(200);
  });

  it('falls back to defaultWidth when the stored value is non-numeric', () => {
    localStorage.setItem('panel-d', 'not-a-number');
    const { result } = renderHook(() => useResizable('panel-d', 200, 100, 400));
    expect(result.current.width).toBe(200);
  });

  it('persists the current width to localStorage on mount', async () => {
    const { result } = renderHook(() => useResizable('panel-e', 200, 100, 400));
    await waitFor(() => expect(localStorage.getItem('panel-e')).toBe('200'));
    expect(localStorage.getItem('panel-e')).toBe(String(result.current.width));
  });

  it('exposes startDrag as a function', () => {
    const { result } = renderHook(() => useResizable('panel-f', 200, 100, 400));
    expect(typeof result.current.startDrag).toBe('function');
  });
});
