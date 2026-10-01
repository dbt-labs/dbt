/**
 * Route table for dbt-docs-v2. Param names (`dbtUniqueId`, `resourceType`)
 * mirror `packages/metadata/dbt-explorer/src/routes.tsx` so a future shared
 * component layer can read the same `useParams` shape in either app.
 *
 * `home` is the project overview — dbt Docs v1 landed on `/overview` too
 * (`$urlRouterProvider.otherwise('/overview')`). The name is kept because it
 * still reads as "the landing page" at every call site.
 */
export const ROUTES = {
  home: '/',
  details: '/details/:dbtUniqueId/',
  resource: '/resource/:resourceType/',
  search: '/search/',
  sourceCollection: '/resource/source/:sourceName/',
  lineage: '/lineage/',
  notFound: '*',
} as const;

export const paths = {
  home: () => '/',
  details: (dbtUniqueId: string) => `/details/${encodeURIComponent(dbtUniqueId)}/`,
  resource: (resourceType: string) => `/resource/${encodeURIComponent(resourceType)}/`,
  search: () => '/search/',
  sourceCollection: (sourceName: string) =>
    `/resource/source/${encodeURIComponent(sourceName)}/`,
  lineage: (dbtUniqueId: string, opts?: { panel?: string }) => {
    const p = new URLSearchParams({ uniqueId: dbtUniqueId });
    if (opts?.panel) p.set('panel', opts.panel);
    return `/lineage/?${p.toString()}`;
  },
};
