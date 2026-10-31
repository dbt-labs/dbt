import { useCallback, useState } from 'react';

function getStorageValue<TValue>(
  key: string,
  validateValue: (value: any) => TValue | null,
  defaultValue: TValue,
) {
  const saved = localStorage.getItem(key);
  if (!saved) return defaultValue;

  try {
    const initial = JSON.parse(saved);
    return validateValue(initial) ?? defaultValue;
  } catch {
    // Handle corrupted data (browser extensions, manual edits, storage bugs)
    return defaultValue;
  }
}

export const useLocalStorage = <TValue>(
  key: string,
  validateValue: (value: any) => TValue | null,
  defaultValue: TValue,
) => {
  const [prevKey, setPrevKey] = useState(key);
  const [value, setValue] = useState(() => {
    return getStorageValue(key, validateValue, defaultValue);
  });

  // Synchronously re-read from localStorage when the key changes.
  // This follows React's recommended pattern for adjusting state when
  // a prop changes, avoiding effects and the stale-closure issues
  // they introduce. React will immediately re-render with the updated
  // value before committing to the DOM — no flicker, no race.
  // See: https://react.dev/learn/you-might-not-need-an-effect#adjusting-some-state-when-a-prop-changes
  if (key !== prevKey) {
    setPrevKey(key);
    setValue(getStorageValue(key, validateValue, defaultValue));
  }

  // Wrap setValue to write to localStorage synchronously, so the write
  // always uses the correct key from the current render closure — no
  // stale-value race between separate read and write effects.
  const setStoredValue = useCallback(
    (action: TValue | ((prev: TValue) => TValue)) => {
      setValue((prev) => {
        const next = action instanceof Function ? action(prev) : action;
        localStorage.setItem(key, JSON.stringify(next));
        return next;
      });
    },
    [key],
  );

  return [value, setStoredValue] as const;
};
