/** Derived view shape — computed from the current URL pathname, used to
 *  drive highlight state in non-routed surfaces (LocatePane, spin trigger).
 *  Mirrors the routes declared in `routes.tsx`. */
export type View =
  | { kind: 'home' }
  | { kind: 'list'; type: string | null }
  | { kind: 'detail'; uniqueId: string };

/** Parse a `react-router-dom` pathname into a `View`. Route table:
 *  `/details/:dbtUniqueId/` → detail; `/resource/:resourceType/` and
 *  `/resource/source/:sourceName/` → list with type; `/search/` → list with
 *  no type; anything else → home (which renders the project overview). */
export function viewFromPath(pathname: string): View {
  const detail = pathname.match(/^\/details\/([^/]+)\/?$/);
  if (detail) return { kind: 'detail', uniqueId: decodeURIComponent(detail[1]) };
  const resource = pathname.match(/^\/resource\/([^/]+?)(?:\/[^/]+)?\/?$/);
  if (resource) return { kind: 'list', type: decodeURIComponent(resource[1]) };
  if (pathname.startsWith('/search')) return { kind: 'list', type: null };
  return { kind: 'home' };
}
