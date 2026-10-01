import { type MouseEvent, type ReactNode } from 'react';
import { Link as RouterLink, type To } from 'react-router-dom';

import { cn } from '../../lib/utils';

export interface LinkProps {
  /** Renders a react-router `<Link>` for in-app navigation, or a plain `<a>` for external URLs. */
  isInternal: boolean;
  to: To;
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
      <RouterLink
        to={to}
        state={state}
        target={resolvedTarget}
        rel={rel}
        onClick={onClick}
        className={linkClassName}
      >
        {children}
      </RouterLink>
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
