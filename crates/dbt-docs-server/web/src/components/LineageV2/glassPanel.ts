/** Shared "glass" treatment for floating DAG canvas panels (bottom bar, minimap).
 *  Simple backdrop-blur/saturate approximation, not a true refraction shader --
 *  see project notes for why that tradeoff was chosen. */
// Tailwind's `/NN` opacity modifier can't decompose a var(--bgMain)-based
// color into RGB channels, so `bg-bgMain/55` silently resolves to alpha 0
// instead of erroring. color-mix() works correctly against CSS custom
// properties, so it's used directly here instead.
export const GLASS_PANEL_CLASSES =
  'rounded-lg border border-borderMuted bg-[color-mix(in_srgb,var(--bgMain)_55%,transparent)] shadow-lg backdrop-blur-xl backdrop-saturate-150 [box-shadow:inset_0_1px_0_rgba(255,255,255,0.08),0_8px_24px_rgba(0,0,0,0.35)]';
