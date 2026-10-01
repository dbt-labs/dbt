/**
 * UI visibility flags. Hardcoded today; the natural next step is to derive
 * each one from a wire-format capability or a build-time env var.
 *
 *  - `hasAnalysis` — analyses are surfaced in the Asset list, File tree, and
 *    Filter pane. Off because analyses have no list projection of their own;
 *    they live on the resource tables so individual detail pages work,
 *    and only the discovery surfaces are gated.
 */
export const FEATURE_FLAGS = {
  hasAnalysis: false,
} as const;
