import {
  type ComponentType,
  createContext,
  type MouseEvent,
  type ReactNode,
  useContext,
} from 'react';

/** An in-app destination: a path, or its parts. */
export type LinkTo = string | { pathname?: string; search?: string; hash?: string };

/** What {@link Navigation.Link} is rendered with. */
export interface NavigationLinkProps {
  to: LinkTo;
  state?: unknown;
  target?: string;
  rel?: string;
  className?: string;
  children: ReactNode;
  onClick?(event: MouseEvent<HTMLAnchorElement>): void;
}

/**
 * The host's pages that components in this package link to. The host builds their
 * paths, since only it knows its URL scheme.
 */
export interface NavigationPaths {
  /** The landing page. */
  home(): string;
  /** A resource's detail page. */
  details(uniqueId: string): string;
  /** The full-screen lineage graph, rooted on a resource. */
  lineage(uniqueId: string): string;
}

/**
 * How components in this package move around the host app. The host owns routing:
 * it supplies the paths of its pages and follows them when asked, so this package
 * carries no router or route table of its own.
 */
export interface Navigation {
  /** Builds the in-app paths that `navigate`, `href` and `Link` take. */
  paths: NavigationPaths;
  /** Client-side navigation to an in-app path. */
  navigate(to: string): void;
  /** The absolute URL of an in-app path, for a link that leaves the app, such as one
   *  copied to the clipboard. Only the host knows how its URLs carry a path. */
  href(to: string): string;
  /** Renders an in-app link, e.g. the host router's `<Link>`. */
  Link: ComponentType<NavigationLinkProps>;
}

const NavigationContext = createContext<Navigation | undefined>(undefined);

export const NavigationProvider = ({
  navigation,
  children,
}: {
  navigation: Navigation;
  children: ReactNode;
}) => (
  <NavigationContext.Provider value={navigation}>{children}</NavigationContext.Provider>
);

/** Read the host's navigation. Throws if no {@link NavigationProvider} is in the
 *  tree — mirrors `useMetadataDataSource`. */
export const useNavigation = (): Navigation => {
  const navigation = useContext(NavigationContext);
  if (navigation === undefined) throw new Error('NavigationProvider not found in tree');
  return navigation;
};
