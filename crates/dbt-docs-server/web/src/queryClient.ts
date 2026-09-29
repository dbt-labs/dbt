import { QueryClient } from '@tanstack/react-query';

/** Shared QueryClient: never refetch on window focus, and retry up to three times.
 *
 *  There used to be a "never retry a 404" carve-out. Nothing raises a status code any
 *  more — a missing resource comes back as `null` from the data source rather than as
 *  a thrown error — so the only failures left to retry are the genuinely transient
 *  ones: fetching a parquet artifact, or loading DuckDB-WASM from the CDN. */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: { refetchOnWindowFocus: false, retry: 3 },
  },
});
