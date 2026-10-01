import { useCallback, useEffect, useState } from 'react';

const STORAGE_KEY = 'dbt-docs-v2:query-history';
const MAX_ENTRIES = 20;

function read(): string[] {
  try {
    const raw = sessionStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((x) => typeof x === 'string') : [];
  } catch {
    return [];
  }
}

/** sessionStorage-backed history of SQL queries the user has run.
 *  Most-recent first, deduplicates consecutive identical entries. */
export function useQueryHistory() {
  const [entries, setEntries] = useState<string[]>(() => read());

  useEffect(() => {
    try {
      sessionStorage.setItem(STORAGE_KEY, JSON.stringify(entries));
    } catch {
      /* ignore */
    }
  }, [entries]);

  const push = useCallback((sql: string) => {
    const trimmed = sql.trim();
    if (!trimmed) return;
    setEntries((prev) => {
      if (prev[0] === trimmed) return prev;
      const next = [trimmed, ...prev.filter((s) => s !== trimmed)].slice(
        0,
        MAX_ENTRIES,
      );
      return next;
    });
  }, []);

  return { entries, push };
}
