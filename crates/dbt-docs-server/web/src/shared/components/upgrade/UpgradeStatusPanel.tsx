import { X } from 'lucide-react';
import { twJoin } from 'tailwind-merge';

import { Button } from '../../../components/ui/Button';
import { InvisibleButton } from '../../../components/ui/InvisibleButton';
import { useLocalStorage } from '../../hooks/useLocalStorage';
import { getUpgradeCopy, isUpgradeCopyVisible } from './copy';
import {
  DecorateOutboundHref,
  identityHref,
  OnUpsellEvent,
  UpgradeHookKind,
  UserState,
} from './types';
import { UpgradeCard } from './UpgradeCard';

/**
 * "Get more from dbt" status panel — the top-level upsell card on the
 * docs-v2 home page. Renders a dismiss X, an optional "Get more from dbt"
 * heading + "Need Enterprise? Contact sales" button, and one
 * {@link UpgradeCard} per provided `(kind, userState)` cell that resolves
 * to a visible row.
 *
 * Density variants:
 *   - `default` — full chrome (heading + contact sales) used by the home
 *     page.
 *   - `compact` — bare body (no heading, no contact-sales button).
 *
 * Dismissal is panel-level and snoozes the entire card for 30 days via
 * localStorage.
 */

const PANEL_BG_STYLE: React.CSSProperties = {
  background: [
    'linear-gradient(172deg, rgba(24, 148, 248, 0) 78%, rgba(24, 148, 248, 0.6) 99%)',
    'linear-gradient(185deg, rgba(99, 47, 245, 0) 68%, rgba(99, 47, 245, 0.6) 97%)',
    'var(--bgMain)',
  ].join(', '),
};

const DISMISSED_STORAGE_KEY = 'dbt:upgrade-status-panel:dismissed-v1';
const SNOOZE_MS = 30 * 24 * 60 * 60 * 1000;
const MAX_ROWS = 4;
const CONTACT_SALES_URL = 'https://www.getdbt.com/contact';

function validateDismissedAt(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function isSnoozeActive(dismissedAt: number | null): boolean {
  if (dismissedAt == null) return false;
  return Date.now() - dismissedAt < SNOOZE_MS;
}

export type UpgradeStatusPanelDensity = 'default' | 'compact';

interface Props {
  /** Ordered list of upgrade hook kinds to consider for rows. Each is
   *  filtered through the copy registry against `userState`; cells that
   *  resolve to `{hidden: true}` are skipped. Clamped to two visible rows. */
  kinds: UpgradeHookKind[];
  userState: UserState;
  /** Visual density. `default` shows badge + heading + contact-sales;
   *  `compact` shows just the badge + rows for embedded banner surfaces. */
  density?: UpgradeStatusPanelDensity;
  /** Override for the contact-sales button URL. */
  contactSalesUrl?: string;
  /** Optional analytics sink. Each visible row fires its own `displayed` /
   *  `clicked`; the panel-level dismiss fires `dismissed` per visible kind.
   *  Consumers that omit it see no behaviour change. */
  onUpsellEvent?: OnUpsellEvent;
  /** Consumer surface, forwarded verbatim as `location`. */
  location?: string;
  /** Decorates outbound hrefs (contact-sales button + each card's CTA /
   *  learn-more) at navigation time — e.g. referral UTM params. Defaults to
   *  identity, so consumers that omit it see no change. */
  decorateOutboundHref?: DecorateOutboundHref;
  className?: string;
  testId?: string;
}

export function UpgradeStatusPanel({
  kinds,
  userState,
  density = 'default',
  contactSalesUrl = CONTACT_SALES_URL,
  onUpsellEvent,
  location,
  decorateOutboundHref = identityHref,
  className,
  testId,
}: Props) {
  const [dismissedAt, setDismissedAt] = useLocalStorage<number | null>(
    DISMISSED_STORAGE_KEY,
    validateDismissedAt,
    null,
  );

  if (isSnoozeActive(dismissedAt)) return null;

  const visibleKinds = kinds
    .filter((kind) => isUpgradeCopyVisible(getUpgradeCopy(kind, userState)))
    .slice(0, MAX_ROWS);

  if (visibleKinds.length === 0) return null;

  const showHeading = density === 'default';

  return (
    <section
      className={twJoin(
        'relative overflow-hidden rounded-lg border border-borderMuted shadow-[0_2px_5px_rgba(0,0,0,0.1)]',
        density === 'compact' ? 'p-2.5' : 'p-3',
        className,
      )}
      aria-label="Get more from dbt"
      data-testid={testId}
    >
      <div
        className="pointer-events-none absolute inset-0 rounded-lg"
        style={PANEL_BG_STYLE}
        aria-hidden
      />
      <div className="relative flex flex-col gap-3">
        <div className="flex items-center justify-end">
          <InvisibleButton
            onClick={() => {
              visibleKinds.forEach((kind) =>
                onUpsellEvent?.({
                  action: 'dismissed',
                  track: kind,
                  location,
                  dismissMethod: 'close-button',
                }),
              );
              setDismissedAt(Date.now());
            }}
            aria-label="Dismiss for 30 days"
            title="Dismiss for 30 days"
            className="inline-flex !w-auto shrink-0 items-center text-fgDecorative hover:text-fgMain"
            testId={testId ? `${testId}-dismiss` : undefined}
          >
            <X className="size-3.5" />
          </InvisibleButton>
        </div>
        {showHeading && (
          <div className="flex items-center justify-between gap-4">
            <h2 className="m-0 text-2xl font-semibold leading-8 text-fgMain">
              Get more from dbt
            </h2>
            <Button
              variant="outline"
              size="sm"
              text="Need Enterprise? Contact sales."
              onClick={() =>
                window.open(decorateOutboundHref(contactSalesUrl), '_blank')
              }
              testId={testId ? `${testId}-contact-sales` : undefined}
            />
          </div>
        )}
        <ul className="m-0 flex list-none flex-col gap-6 p-0">
          {visibleKinds.map((kind) => (
            <li key={kind}>
              <UpgradeCard
                kind={kind}
                userState={userState}
                variant="inline"
                onUpsellEvent={onUpsellEvent}
                location={location}
                decorateOutboundHref={decorateOutboundHref}
                promptFormat="panel-row"
                testId={testId ? `${testId}-row-${kind}` : undefined}
              />
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
