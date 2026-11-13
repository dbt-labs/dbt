import { useCallback, useEffect, useState } from 'react';

export type Theme = 'dark' | 'light' | 'system';
const STORAGE_KEY = 'dbt-docs-v2:theme';
const DEFAULT: Theme = 'dark';

function read(): Theme {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw === 'light' || raw === 'dark' || raw === 'system' ? raw : DEFAULT;
  } catch {
    return DEFAULT;
  }
}

function systemPrefersDark(): boolean {
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? true;
}

export function useTheme() {
  const [theme, setTheme] = useState<Theme>(() => read());
  // Only used to force a re-resolve when the OS preference flips while
  // theme === 'system' — the applied class is still derived fresh below.
  const [systemDark, setSystemDark] = useState<boolean>(() => systemPrefersDark());

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => setSystemDark(media.matches);
    media.addEventListener('change', onChange);
    return () => media.removeEventListener('change', onChange);
  }, []);

  const resolved: 'dark' | 'light' =
    theme === 'system' ? (systemDark ? 'dark' : 'light') : theme;

  useEffect(() => {
    const root = document.documentElement;
    root.classList.toggle('dark', resolved === 'dark');
    root.classList.toggle('light', resolved === 'light');
    try {
      localStorage.setItem(STORAGE_KEY, theme);
    } catch {
      // Storage unavailable; in-memory only.
    }
  }, [theme, resolved]);

  const toggle = useCallback(() => {
    setTheme((t) => (t === 'dark' ? 'light' : 'dark'));
  }, []);

  return { theme, resolved, toggle, setTheme };
}
