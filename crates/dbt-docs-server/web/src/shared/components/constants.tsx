import { BadgeAlert, BadgeCheck, BadgeMinus } from 'lucide-react';

import { TrustState } from '../util/trustSignals';

/** Matches sourdough's SizeType structurally (same string values) so it's
 *  still assignable everywhere that type was used -- TypeScript checks
 *  shape, not origin, for a plain string-literal union. */
type SourdoughSize = 'xs' | 'sm' | 'md' | 'lg' | 'xl' | '2xl' | '3xl' | '4xl';

/** Mirrors sourdough's rendered ryecon sizes (Ryecons.css custom properties). */
const SIZE_CLASS: Record<SourdoughSize, string> = {
  xs: 'size-3',
  sm: 'size-3.5',
  md: 'size-4',
  lg: 'size-5',
  xl: 'size-6',
  '2xl': 'size-8',
  '3xl': 'size-10',
  '4xl': 'size-12',
};

export const trustIconMap = (
  size: SourdoughSize,
): Record<TrustState, React.ReactElement> => {
  const sizeClass = SIZE_CLASS[size];
  return {
    healthy: (
      <BadgeCheck data-testid="healthy-icon" className={`mt-[2px] ${sizeClass}`} />
    ),
    caution: (
      <BadgeAlert data-testid="caution-icon" className={`mt-[2px] ${sizeClass}`} />
    ),
    degraded: (
      <BadgeAlert data-testid="degraded-icon" className={`mt-[2px] ${sizeClass}`} />
    ),
    unknown: (
      <BadgeMinus data-testid="unknown-icon" className={`mt-[2px] ${sizeClass}`} />
    ),
  };
};
