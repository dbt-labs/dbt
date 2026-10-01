import { useState } from 'react';
import { twJoin } from 'tailwind-merge';

import { useLocalStorage } from '../../hooks/useLocalStorage';
import { getUpgradeCopy, isUpgradeCopyVisible } from './copy';
import {
  ALL_UPGRADE_HOOK_KINDS,
  DecorateOutboundHref,
  OnUpsellEvent,
  UpgradeHookKind,
  UserState,
} from './types';
import { UpgradeCard } from './UpgradeCard';

/**
 * Side-panel rail-footer card stack used in the dbt-docs-v2 sidebar. Up to
 * two cards are visible at once, one expanded and the rest collapsed.
 * Clicking a collapsed card opens it and collapses the previously open
 * card. The X is a separate dismiss control: it snoozes the card for 30
 * days and persists via localStorage.
 *
 * The visible kinds are filtered through the shared copy registry — cells
 * that resolve to `{hidden: true}` for the given `userState` are dropped,
 * so callers can pass a wide pool and let gating filter per state.
 *
 * Mirrors Figma OMejLgivWDSbjaEpEdWmf6 node 18554:1148.
 */

const DISMISSED_STORAGE_KEY = 'dbt:upgrade-rail:dismissed-v3';
const SNOOZE_MS = 30 * 24 * 60 * 60 * 1000;
const DEFAULT_MAX_VISIBLE = 2;

type DismissedMap = Partial<Record<UpgradeHookKind, number>>;

function validateDismissed(value: unknown): DismissedMap | null {
  if (!value || typeof value !== 'object') return null;
  const input = value as Record<string, unknown>;
  const out: DismissedMap = {};
  ALL_UPGRADE_HOOK_KINDS.forEach((k) => {
    const ts = input[k];
    if (typeof ts === 'number' && Number.isFinite(ts)) out[k] = ts;
  });
  return out;
}

function isSnoozeActive(dismissedAt: number | undefined): boolean {
  if (dismissedAt == null) return false;
  return Date.now() - dismissedAt < SNOOZE_MS;
}

interface Props {
  /** Ordered list of upgrade hook kinds to consider. Each is filtered
   *  through the copy registry against `userState`; cells that resolve to
   *  `{hidden: true}` are dropped. Defaults to the full pool. */
  kinds?: UpgradeHookKind[];
  userState: UserState;
  /** Cap on visible cards. Defaults to two per Notion handoff. */
  maxVisible?: number;
  /** Which card starts expanded. Defaults to the first visible card.
   *  Pass `null` to start fully collapsed. */
  defaultOpenKey?: UpgradeHookKind | null;
  /** Override the localStorage key used to persist dismissals. Helpful
   *  when multiple rail surfaces should snooze independently. */
  dismissedStorageKey?: string;
  /** Optional analytics sink. Each card fires its own `displayed` / `clicked`;
   *  dismissing a card fires `dismissed`. Consumers that omit it see no
   *  behaviour change. */
  onUpsellEvent?: OnUpsellEvent;
  /** Consumer surface, forwarded verbatim as `location`. */
  location?: string;
  /** Decorates outbound CTA / learn-more hrefs on each card (e.g. referral
   *  UTM params). Defaults to identity, so consumers that omit it see no
   *  change. */
  decorateOutboundHref?: DecorateOutboundHref;
  className?: string;
}

export function UpgradeRailStack({
  kinds = ALL_UPGRADE_HOOK_KINDS,
  userState,
  maxVisible = DEFAULT_MAX_VISIBLE,
  defaultOpenKey,
  dismissedStorageKey = DISMISSED_STORAGE_KEY,
  onUpsellEvent,
  location,
  decorateOutboundHref,
  className,
}: Props) {
  const [dismissed, setDismissed] = useLocalStorage<DismissedMap>(
    dismissedStorageKey,
    validateDismissed,
    {},
  );

  const visibleKinds = kinds
    .filter((kind) => isUpgradeCopyVisible(getUpgradeCopy(kind, userState)))
    .filter((kind) => !isSnoozeActive(dismissed[kind]))
    .slice(0, maxVisible);

  const resolvedDefaultOpen =
    defaultOpenKey === undefined ? (visibleKinds[0] ?? null) : defaultOpenKey;
  const initialOpen =
    resolvedDefaultOpen && visibleKinds.includes(resolvedDefaultOpen)
      ? resolvedDefaultOpen
      : null;

  const [openKey, setOpenKey] = useState<UpgradeHookKind | null>(initialOpen);

  if (visibleKinds.length === 0) return null;

  const handleDismiss = (kind: UpgradeHookKind) => {
    onUpsellEvent?.({
      action: 'dismissed',
      track: kind,
      format: 'rail-card',
      location,
      dismissMethod: 'close-button',
    });
    setDismissed((prev) => ({ ...prev, [kind]: Date.now() }));
    if (openKey === kind) setOpenKey(null);
  };

  return (
    <div className={twJoin('flex flex-col gap-2', className)}>
      {visibleKinds.map((kind) => {
        const isOpen = openKey === kind;
        return (
          <UpgradeCard
            key={kind}
            kind={kind}
            userState={userState}
            variant={isOpen ? 'rail-expanded' : 'rail-collapsed'}
            onToggleExpand={() => setOpenKey(isOpen ? null : kind)}
            onDismiss={() => handleDismiss(kind)}
            onUpsellEvent={onUpsellEvent}
            location={location}
            decorateOutboundHref={decorateOutboundHref}
            testId={`upgrade-rail-card-${kind}`}
          />
        );
      })}
    </div>
  );
}
