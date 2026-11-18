/**
 * Lends this app's router to `dbt-docs-lib`.
 *
 * The app owns routing; the lib has no router or route table of its own. Its
 * components build destinations with the app's `paths` and call `navigate`, or render
 * `Link`, and this hands both to react-router, so they navigate in the same history as
 * the rest of the app. `href` turns a destination into a full URL for links that leave
 * the app.
 */

import { type ReactNode, useMemo } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { type Navigation, NavigationProvider } from 'dbt-docs-lib';

import { paths } from '../routes';

/** Must sit inside the router. */
export function RouterNavigationProvider({ children }: { children: ReactNode }) {
  const navigate = useNavigate();
  const navigation = useMemo<Navigation>(
    () => ({
      paths,
      navigate: (to) => navigate(to),
      // The app routes on the hash (`HashRouter` in `main.tsx`), so the path goes
      // after the `#` of the page's own URL.
      href: (to) => new URL(`#${to}`, window.location.href).href,
      Link,
    }),
    [navigate],
  );
  return <NavigationProvider navigation={navigation}>{children}</NavigationProvider>;
}
