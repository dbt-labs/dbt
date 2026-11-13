/** A single facet option: the value plus its asset count (null when the
 *  backend doesn't compute one). */
export interface FacetValue {
  value: string;
  count: number | null;
}

/** Facet options keyed by the {@link AssetFilter} field they drive (e.g.
 *  `'owners'`, `'packages'`, `'results'`, `'testTypes'`, `'modelingLayers'`).
 *  Consumers render one dropdown per key and feed the selected value back into
 *  the matching filter field. */
export type Facets = Record<string, FacetValue[]>;
