import { FC, useEffect, useRef } from 'react';
import { twJoin } from 'tailwind-merge';

import { Button } from '../../../components/ui/Button';
import { getUpgradeCopy, isUpgradeCopyVisible } from './copy';
import {
  DecorateOutboundHref,
  identityHref,
  OnUpsellEvent,
  UpgradeHookKind,
  UserState,
} from './types';

/**
 * Row-level upsell. Used inside dense, key/value-style sections (e.g.
 * an asset's Metadata table) to point at a field that would exist if
 * the user upgraded. The row label describes the absent field; the CTA
 * comes from the `(kind, userState)` registry.
 */
interface Props {
  /** Label for the absent field — e.g. "Consumption queries (excludes builds)". */
  label: string;
  kind: UpgradeHookKind;
  userState: UserState;
  /** Optional analytics sink. Fires `displayed` on mount and `clicked` on the
   *  CTA. Consumers that omit it see no behaviour change. */
  onUpsellEvent?: OnUpsellEvent;
  /** Consumer surface, forwarded verbatim as `location`. */
  location?: string;
  /** Decorates the CTA href at click time (e.g. referral UTM params).
   *  Defaults to identity, so consumers that omit it see no change. */
  decorateOutboundHref?: DecorateOutboundHref;
  testId?: string;
  className?: string;
}

export const UpgradeRow: FC<Props> = ({
  label,
  kind,
  userState,
  onUpsellEvent,
  location,
  decorateOutboundHref = identityHref,
  testId,
  className,
}) => {
  const spec = getUpgradeCopy(kind, userState);
  const visible = isUpgradeCopyVisible(spec) && spec.cta.kind === 'button';

  const displayedRef = useRef(false);
  useEffect(() => {
    if (!visible || displayedRef.current) return;
    displayedRef.current = true;
    onUpsellEvent?.({ action: 'displayed', track: kind, format: 'row', location });
  }, [visible, kind, location, onUpsellEvent]);

  if (!isUpgradeCopyVisible(spec)) return null;
  if (spec.cta.kind !== 'button') return null;

  const { label: ctaLabel, href } = spec.cta;

  return (
    <div className={twJoin('flex flex-wrap', className)} data-testid={testId}>
      <span className="my-5 flex w-full items-center justify-between gap-3">
        <span className="min-w-0 flex-1 truncate text-fgDecorative">{label}</span>
        <span className="flex shrink-0 items-center gap-2">
          <Button
            variant="default"
            size="sm"
            text={ctaLabel}
            onClick={() => {
              onUpsellEvent?.({
                action: 'clicked',
                track: kind,
                format: 'row',
                location,
                ctaLabel,
              });
              window.open(decorateOutboundHref(href), '_blank');
            }}
            testId={testId ? `${testId}-cta` : undefined}
          />
        </span>
      </span>
    </div>
  );
};
