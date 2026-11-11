import { type MouseEvent, type ReactNode } from 'react';

import {
  type LinkTo,
  type NavigationLinkProps,
  useNavigation,
} from '../../context/NavigationProvider';
import { cn } from '../../utils';

export interface LinkProps {
  /** Renders the host's link (from `NavigationProvider`) for in-app navigation, or a
   *  plain `<a>` for external URLs. */
  isInternal: boolean;
  to: LinkTo;
  state?: unknown;
  target?: string;
  shouldOpenNewTab?: boolean;
  hideUnderline?: boolean;
  className?: string;
  children: ReactNode;
  onClick?(event: MouseEvent<HTMLAnchorElement>): void;
}

export function Link({
  isInternal,
  to,
  state,
  target,
  shouldOpenNewTab,
  hideUnderline,
  className,
  children,
  onClick,
}: LinkProps) {
  const resolvedTarget = shouldOpenNewTab ? '_blank' : target;
  const rel = resolvedTarget === '_blank' ? 'noopener noreferrer' : undefined;
  const linkClassName = cn(
    'text-fgBrand underline-offset-4',
    !hideUnderline && 'underline',
    className,
  );

  if (isInternal) {
    return (
      <InternalLink
        to={to}
        state={state}
        target={resolvedTarget}
        rel={rel}
        onClick={onClick}
        className={linkClassName}
      >
        {children}
      </InternalLink>
    );
  }

  const href = typeof to === 'string' ? to : (to.pathname ?? '');
  return (
    <a
      href={href}
      target={resolvedTarget}
      rel={rel}
      onClick={onClick}
      className={linkClassName}
    >
      {children}
    </a>
  );
}

/** Split out so only internal links need a `NavigationProvider` above them. */
function InternalLink(props: NavigationLinkProps) {
  const { Link: HostLink } = useNavigation();
  return <HostLink {...props} />;
}
