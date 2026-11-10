import { useCallback, useEffect, useState } from 'react';

/** Persist a width to localStorage so the user's resize choice survives
 *  reloads. Min/max clamp prevents pathological drags. */
export function useResizable(
  key: string,
  defaultWidth: number,
  min: number,
  max: number,
) {
  const [width, setWidth] = useState<number>(() => {
    try {
      const raw = localStorage.getItem(key);
      if (raw) {
        const n = parseInt(raw, 10);
        if (!Number.isNaN(n) && n >= min && n <= max) return n;
      }
    } catch {
      /* ignore */
    }
    return defaultWidth;
  });

  useEffect(() => {
    try {
      localStorage.setItem(key, String(width));
    } catch {
      /* ignore */
    }
  }, [key, width]);

  const startDrag = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      e.preventDefault();
      const startX = e.clientX;
      const startW = width;
      const onMove = (ev: PointerEvent) => {
        const next = Math.min(max, Math.max(min, startW + (ev.clientX - startX)));
        setWidth(next);
      };
      const onUp = () => {
        window.removeEventListener('pointermove', onMove);
        window.removeEventListener('pointerup', onUp);
        document.body.style.cursor = '';
        document.body.style.userSelect = '';
      };
      window.addEventListener('pointermove', onMove);
      window.addEventListener('pointerup', onUp);
      document.body.style.cursor = 'col-resize';
      document.body.style.userSelect = 'none';
    },
    [width, min, max],
  );

  return { width, startDrag };
}
