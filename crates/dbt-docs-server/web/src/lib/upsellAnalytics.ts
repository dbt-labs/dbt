import type { UpsellAnalyticsEvent } from '../shared';
import { trackReferralLinkClicked, trackUpsellEvent } from './telemetry';

/**
 * Map a shared {@link UpsellAnalyticsEvent} from the metadata-shared upsell
 * components onto the docs-v2 analytics wire events. A single handler is wired
 * at every upsell render site (see App / LocatePane / NodeDetail).
 *
 * The `clicked` action fans out: a CTA button carries `ctaLabel` →
 * `upsell_prompt_clicked`; an inline learn-more / external doc link carries
 * `destination` (no label) → `referral_link_clicked`. There is no referral
 * code in the v1 copy, so `referral_code` is always empty.
 */
export function handleUpsellEvent(event: UpsellAnalyticsEvent): void {
  switch (event.action) {
    case 'displayed':
      trackUpsellEvent({
        event_type: 'upsell_prompt_displayed',
        upsell_track: event.track,
        prompt_format: event.format ?? '',
        prompt_location: event.location ?? '',
      });
      return;
    case 'clicked':
      if (event.ctaLabel != null) {
        trackUpsellEvent({
          event_type: 'upsell_prompt_clicked',
          upsell_track: event.track,
          cta_label: event.ctaLabel,
          referral_code: '',
        });
      } else if (event.destination != null) {
        trackReferralLinkClicked({
          referral_code: '',
          link_destination: event.destination,
        });
      }
      return;
    case 'dismissed':
      trackUpsellEvent({
        event_type: 'upsell_prompt_dismissed',
        upsell_track: event.track,
        dismiss_method: event.dismissMethod ?? '',
      });
      return;
  }
}
