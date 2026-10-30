import { FC, useEffect, useRef } from 'react';
import { ChevronUp, X } from 'lucide-react';
import { twJoin } from 'tailwind-merge';

import { Button } from '../../../components/ui/Button';
import { InvisibleButton } from '../../../components/ui/InvisibleButton';
import { Link } from '../../../components/ui/Link';
import { getUpgradeCopy, isUpgradeCopyVisible } from './copy';
import { CopyCommandSnippet } from './CopyCommandSnippet';
import {
  CopySpec,
  DecorateOutboundHref,
  identityHref,
  OnUpsellEvent,
  UpgradeHookKind,
  UserState,
} from './types';

/**
 * Four visual variants of a single upsell card:
 *
 *   - `inline` — wide, single-row card. Status dot · title · description ·
 *     CTA (snippet or button). Used by `UpgradeStatusPanel` rows and the
 *     asset-detail banner.
 *   - `block` — large, two-row block. Dismiss X · h2 headline · body
 *     paragraph with inline learn-more link. Used by the home-page
 *     persistent dbt State card.
 *   - `rail-expanded` — small rail card with a dbt-logo header, dismiss X,
 *     description body, and CTA. Used by `UpgradeRailStack` when open.
 *   - `rail-collapsed` — small rail card with a dbt-logo header, subtitle,
 *     and expand caret. Used by `UpgradeRailStack` when closed.
 *
 * The card is presentational and stateless. Dismissal / expansion are
 * driven by `onDismiss` / `onToggleExpand` callbacks supplied by the
 * higher-level panel and rail stack.
 *
 * If the `(kind, userState)` cell resolves to `{ hidden: true }`, the
 * card renders nothing. Callers can pass a wide kind pool and rely on
 * the registry to filter per user state.
 */
export type UpgradeCardVariant =
  'inline' | 'block' | 'rail-expanded' | 'rail-collapsed';

interface BaseProps {
  kind: UpgradeHookKind;
  userState: UserState;
  variant: UpgradeCardVariant;
  /** When provided, renders the dismiss X. Persistence is the caller's job. */
  onDismiss?: () => void;
  /** Required for `rail-*` variants; ignored on `inline`. */
  onToggleExpand?: () => void;
  /** Optional analytics sink. Fires `displayed` on mount and `clicked` on the
   *  CTA / learn-more links. Consumers that omit it see no behaviour change. */
  onUpsellEvent?: OnUpsellEvent;
  /** Optional decorator applied to outbound CTA / learn-more hrefs at the point
   *  of navigation. Defaults to identity, so consumers that omit it (e.g.
   *  dbt-explorer) see no behaviour change. */
  decorateOutboundHref?: DecorateOutboundHref;
  /** Consumer surface, forwarded verbatim as `location` on every event. */
  location?: string;
  /** Override the derived `format`. Used by `UpgradeStatusPanel` so its rows
   *  report `panel-row` rather than the standalone `card` default. */
  promptFormat?: string;
  testId?: string;
  className?: string;
}

/** Derived prompt format per variant. Overridable via `promptFormat`. */
const VARIANT_FORMAT: Record<UpgradeCardVariant, string> = {
  inline: 'card',
  block: 'block',
  'rail-expanded': 'rail-card',
  'rail-collapsed': 'rail-card',
};

export const UpgradeCard: FC<BaseProps> = ({
  kind,
  userState,
  variant,
  onDismiss,
  onToggleExpand,
  onUpsellEvent,
  decorateOutboundHref = identityHref,
  location,
  promptFormat,
  testId,
  className,
}) => {
  const spec = getUpgradeCopy(kind, userState);
  const visible = isUpgradeCopyVisible(spec);
  const format = promptFormat ?? VARIANT_FORMAT[variant];

  // `displayed`, once per mounted+visible prompt. Guarded so React re-renders
  // and StrictMode double-invokes don't re-emit. Rail collapsed→expanded is
  // the same mounted card, so it counts as one display.
  const displayedRef = useRef(false);
  useEffect(() => {
    if (!visible || displayedRef.current) return;
    displayedRef.current = true;
    onUpsellEvent?.({ action: 'displayed', track: kind, format, location });
  }, [visible, kind, format, location, onUpsellEvent]);

  if (!visible) return null;

  const showDismiss = Boolean(onDismiss);
  const emit = onUpsellEvent
    ? {
        onCtaClick: (ctaLabel: string) =>
          onUpsellEvent({ action: 'clicked', track: kind, format, location, ctaLabel }),
        onLearnMore: (destination: string) =>
          onUpsellEvent({
            action: 'clicked',
            track: kind,
            format,
            location,
            destination,
          }),
      }
    : undefined;

  switch (variant) {
    case 'inline':
      return (
        <InlineCard
          spec={spec}
          showDismiss={showDismiss}
          onDismiss={onDismiss}
          emit={emit}
          decorate={decorateOutboundHref}
          testId={testId}
          className={className}
        />
      );
    case 'block':
      return (
        <BlockCard
          spec={spec}
          showDismiss={showDismiss}
          onDismiss={onDismiss}
          emit={emit}
          decorate={decorateOutboundHref}
          testId={testId}
          className={className}
        />
      );
    case 'rail-expanded':
      return (
        <RailCard
          kind={kind}
          spec={spec}
          isOpen
          showDismiss={showDismiss}
          onDismiss={onDismiss}
          onToggleExpand={onToggleExpand}
          emit={emit}
          decorate={decorateOutboundHref}
          testId={testId}
          className={className}
        />
      );
    case 'rail-collapsed':
      return (
        <RailCard
          kind={kind}
          spec={spec}
          isOpen={false}
          showDismiss={false}
          onToggleExpand={onToggleExpand}
          emit={emit}
          decorate={decorateOutboundHref}
          testId={testId}
          className={className}
        />
      );
  }
};

/** Click sinks handed to the presentational sub-cards. */
interface CardEmit {
  onCtaClick: (ctaLabel: string) => void;
  onLearnMore: (destination: string) => void;
}

/* ---------- inline (status-panel row + standalone) ---------- */

interface InlineProps {
  spec: Extract<CopySpec, { hidden?: false }>;
  showDismiss: boolean;
  onDismiss?: () => void;
  emit?: CardEmit;
  decorate: DecorateOutboundHref;
  testId?: string;
  className?: string;
}

function InlineCard({
  spec,
  showDismiss,
  onDismiss,
  emit,
  decorate,
  testId,
  className,
}: InlineProps) {
  return (
    <div
      className={twJoin(
        'relative flex items-center justify-between gap-4 overflow-hidden rounded border border-borderMuted bg-bgMain px-4 py-3',
        className,
      )}
      data-testid={testId}
    >
      <div className="flex min-w-0 flex-1 items-center gap-4">
        <span
          className={twJoin(
            'h-2.5 w-2.5 shrink-0 rounded-full',
            spec.onState ? 'bg-bgDuotoneSuccess' : 'bg-fgDecorative',
          )}
          aria-hidden
        />
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <div className="text-sm font-semibold leading-5 text-fgMain">
            {spec.onState ? `${spec.title} is ON` : spec.title}
          </div>
          <p className="m-0 max-w-[680px] text-[13px] leading-[18px] text-fgDecorative">
            {spec.description}
            {spec.learnMore && (
              <>
                {' '}
                <Link
                  isInternal={false}
                  to={decorate(spec.learnMore.href)}
                  shouldOpenNewTab
                  onClick={() => emit?.onLearnMore(spec.learnMore!.href)}
                  className="!text-fgBrand underline underline-offset-4 hover:!text-fgBrandHover"
                >
                  {spec.learnMore.label}
                </Link>
              </>
            )}
          </p>
        </div>
      </div>
      {spec.cta.kind !== 'none' && (
        <CtaForSpec spec={spec} emit={emit} decorate={decorate} testId={testId} />
      )}
      {showDismiss && onDismiss && (
        <InvisibleButton
          onClick={onDismiss}
          aria-label="Dismiss"
          title="Dismiss"
          className="inline-flex !w-auto shrink-0 items-center text-fgDecorative hover:text-fgMain"
          testId={testId ? `${testId}-dismiss` : undefined}
        >
          <X className="size-3.5" />
        </InvisibleButton>
      )}
    </div>
  );
}

/* ---------- block (home-page persistent card) ---------- */

interface BlockProps {
  spec: Extract<CopySpec, { hidden?: false }>;
  showDismiss: boolean;
  onDismiss?: () => void;
  emit?: CardEmit;
  decorate: DecorateOutboundHref;
  testId?: string;
  className?: string;
}

function BlockCard({
  spec,
  showDismiss,
  onDismiss,
  emit,
  decorate,
  testId,
  className,
}: BlockProps) {
  const heading = spec.headline ?? spec.title;
  return (
    <section
      className={twJoin(
        'flex flex-col gap-3 rounded-lg border border-borderMuted bg-bgMain p-4',
        className,
      )}
      data-testid={testId}
    >
      <div className="flex items-center justify-end gap-2">
        {showDismiss && onDismiss && (
          <InvisibleButton
            onClick={onDismiss}
            aria-label={`Dismiss ${spec.title}`}
            title="Dismiss"
            className="inline-flex !w-auto shrink-0 items-center text-fgDecorative hover:text-fgMain"
            testId={testId ? `${testId}-dismiss` : undefined}
          >
            <X className="size-3.5" />
          </InvisibleButton>
        )}
      </div>
      <h2 className="m-0 text-2xl font-semibold leading-8 text-fgMain">{heading}</h2>
      <p className="m-0 text-sm leading-5 text-fgDecorative">
        {spec.description}
        {spec.learnMore && (
          <>
            {' '}
            <Link
              isInternal={false}
              to={decorate(spec.learnMore.href)}
              shouldOpenNewTab
              onClick={() => emit?.onLearnMore(spec.learnMore!.href)}
              className="!text-fgBrand underline underline-offset-4 hover:!text-fgBrandHover"
            >
              {spec.learnMore.label}
            </Link>
          </>
        )}
      </p>
    </section>
  );
}

/* ---------- rail (collapsed + expanded) ---------- */

interface RailProps {
  kind: UpgradeHookKind;
  spec: Extract<CopySpec, { hidden?: false }>;
  isOpen: boolean;
  showDismiss: boolean;
  onDismiss?: () => void;
  onToggleExpand?: () => void;
  emit?: CardEmit;
  decorate: DecorateOutboundHref;
  testId?: string;
  className?: string;
}

function RailCard({
  kind,
  spec,
  isOpen,
  showDismiss,
  onDismiss,
  onToggleExpand,
  emit,
  decorate,
  testId,
  className,
}: RailProps) {
  const bodyId = `upgrade-card-${kind}-body`;
  return (
    <article
      className={twJoin(
        'flex flex-col gap-2 rounded-lg border border-borderMuted bg-bgMain px-3.5 py-3',
        className,
      )}
      data-testid={testId}
    >
      <div className="flex items-center gap-2">
        <InvisibleButton
          onClick={onToggleExpand ?? (() => undefined)}
          aria-expanded={isOpen}
          aria-controls={bodyId}
          className="flex !w-auto min-w-0 flex-1 items-center gap-2 text-left text-fgMain"
          testId={testId ? `${testId}-toggle` : undefined}
        >
          <span className="min-w-0 flex-1 text-[13px] font-bold leading-[18px] text-fgMain">
            {spec.title}
          </span>
          {!isOpen && (
            <span
              className="inline-flex shrink-0 items-center text-fgDecorative"
              aria-hidden
            >
              <ChevronUp className="size-3" />
            </span>
          )}
        </InvisibleButton>
        {isOpen && showDismiss && onDismiss && (
          <InvisibleButton
            onClick={onDismiss}
            aria-label={`Dismiss ${spec.title}`}
            title="Dismiss"
            className="inline-flex !w-auto shrink-0 items-center text-fgDecorative hover:text-fgMain"
            testId={testId ? `${testId}-dismiss` : undefined}
          >
            <X className="size-3" />
          </InvisibleButton>
        )}
      </div>
      <div id={bodyId} className={twJoin('flex flex-col gap-3', !isOpen && 'pl-6')}>
        {isOpen ? (
          <>
            <p className="m-0 max-w-[680px] text-[12px] leading-4 text-fgAlt">
              {spec.description}
            </p>
            {spec.cta.kind !== 'none' && (
              <CtaForSpec
                spec={spec}
                emit={emit}
                decorate={decorate}
                fullWidth
                testId={testId}
              />
            )}
          </>
        ) : (
          <p className="m-0 max-w-[680px] text-[12px] leading-4 text-fgDecorative">
            {spec.subtitle}
          </p>
        )}
      </div>
    </article>
  );
}

/* ---------- CTA shared between variants ---------- */

interface CtaProps {
  spec: Extract<CopySpec, { hidden?: false }>;
  emit?: CardEmit;
  decorate: DecorateOutboundHref;
  fullWidth?: boolean;
  testId?: string;
}

function CtaForSpec({ spec, emit, decorate, fullWidth, testId }: CtaProps) {
  if (spec.cta.kind === 'snippet') {
    return (
      <CopyCommandSnippet
        command={spec.cta.command}
        className={fullWidth ? 'self-center' : undefined}
      />
    );
  }
  if (spec.cta.kind === 'none') return null;
  const { label, href } = spec.cta;
  return (
    <Button
      variant="default"
      size="sm"
      text={label}
      className={fullWidth ? 'w-full justify-center' : undefined}
      onClick={() => {
        emit?.onCtaClick(label);
        window.open(decorate(href), '_blank');
      }}
      testId={testId ? `${testId}-cta` : undefined}
    />
  );
}
