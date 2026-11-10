import { afterEach, describe, expect, it } from 'vitest';

import { type Identity } from '../types';
import { decorateOutboundHref } from './outboundReferrer';
import { initTelemetry, resetTelemetryForTests } from './telemetry';

const ENABLED: Identity = { is_logged_in: true, analytics_enabled: true };
const DISABLED: Identity = { is_logged_in: true, analytics_enabled: false };

const UTM = 'utm_source=dbt-docs-v2&utm_medium=referral';

describe('decorateOutboundHref', () => {
  afterEach(() => resetTelemetryForTests());

  describe('with consent', () => {
    it('appends referral UTM params to allowlisted hosts', () => {
      initTelemetry(ENABLED);

      expect(decorateOutboundHref('https://www.getdbt.com/contact')).toBe(
        `https://www.getdbt.com/contact?${UTM}`,
      );
      expect(decorateOutboundHref('https://state.dbt.com/')).toBe(
        `https://state.dbt.com/?${UTM}`,
      );
      expect(decorateOutboundHref('https://docs.getdbt.com/docs/build')).toBe(
        `https://docs.getdbt.com/docs/build?${UTM}`,
      );
    });

    it('preserves the existing query and hash, merging UTM into the query', () => {
      initTelemetry(ENABLED);

      expect(
        decorateOutboundHref(
          'https://docs.getdbt.com/docs/build/view-documentation?version=2.0#dbt-docs-v2',
        ),
      ).toBe(
        `https://docs.getdbt.com/docs/build/view-documentation?version=2.0&${UTM}#dbt-docs-v2`,
      );
    });

    it('leaves off-allowlist hosts untouched', () => {
      initTelemetry(ENABLED);

      const href = 'https://example.com/page?keep=1';
      expect(decorateOutboundHref(href)).toBe(href);
    });

    it('leaves non-absolute / unparseable hrefs untouched', () => {
      initTelemetry(ENABLED);

      expect(decorateOutboundHref('/relative/path')).toBe('/relative/path');
      expect(decorateOutboundHref('#anchor')).toBe('#anchor');
      expect(decorateOutboundHref('not a url')).toBe('not a url');
    });
  });

  describe('without consent', () => {
    it('returns the href untouched when telemetry is not initialised', () => {
      const href = 'https://www.getdbt.com/contact';
      expect(decorateOutboundHref(href)).toBe(href);
    });

    it('returns the href untouched when analytics is not consented', () => {
      initTelemetry(DISABLED);

      const href = 'https://www.getdbt.com/contact';
      expect(decorateOutboundHref(href)).toBe(href);
    });
  });
});
