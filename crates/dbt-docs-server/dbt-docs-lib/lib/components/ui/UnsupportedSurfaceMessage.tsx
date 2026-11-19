/**
 * What to show when the active data source does not implement a surface.
 *
 * A `MetadataDataSource` advertises what it supports by which optional fetchers
 * it defines, and the hooks disable themselves when one is absent. That keeps a
 * missing surface from erroring — but "disabled" renders as an *empty* list, and
 * an empty list is the one thing this must not look like: beside a sidebar count
 * of 3, "no models" reads as data loss rather than as a view that is not wired up
 * yet.
 *
 * So unsupported surfaces speak through the same channel as a real error, which
 * every list table already renders. Nothing to add per call site.
 */
export const UNSUPPORTED_SURFACE_MESSAGE =
  'Not available in this docs site yet — this view is still being ported to run in the browser.';
