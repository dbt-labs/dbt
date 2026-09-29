import { useEffect, useMemo, useState } from 'react';
import { twMerge } from 'tailwind-merge';

import { type Breadcrumb } from '../../components/ui/Breadcrumb';
import { Link } from '../../components/ui/Link';
import { truthy } from '../util/array';

interface SimpleLinkBreadcrumbsProps extends React.ComponentPropsWithoutRef<'span'> {
  breadcrumbs: Omit<Breadcrumb, 'skeleton'>[];
}

export const SimpleLinkBreadcrumbs = ({
  breadcrumbs,
  className,
  ...spanProps
}: SimpleLinkBreadcrumbsProps) => {
  const [containerEl, setContainerEl] = useState<HTMLSpanElement | null>(null);
  const [isHovered, updateIsHovered] = useState(false);
  const [isFocused, updateIsFocused] = useState(false);
  const [doesOverflow, setDoesOverflow] = useState(false);

  useEffect(() => {
    if (!containerEl) return;
    const measure = () => {
      setDoesOverflow(containerEl.scrollWidth > containerEl.clientWidth);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(containerEl);
    return () => observer.disconnect();
  }, [containerEl]);

  const shouldTruncate = !isHovered && !isFocused && doesOverflow;

  const displayedBreadcrumbs = useMemo((): Omit<Breadcrumb, 'skeleton'>[] => {
    if (shouldTruncate) {
      return [{ text: '..' }, ...breadcrumbs].filter(truthy);
    }
    return breadcrumbs;
  }, [breadcrumbs, shouldTruncate]);

  const numBreadcrumbs = displayedBreadcrumbs.length;

  return (
    <span
      {...spanProps}
      className={twMerge(className, shouldTruncate && 'truncate')}
      ref={setContainerEl}
      onMouseEnter={() => {
        updateIsHovered(true);
      }}
      onMouseLeave={() => {
        updateIsHovered(false);
      }}
      onFocusCapture={() => {
        updateIsFocused(true);
      }}
      onBlurCapture={() => {
        updateIsFocused(false);
      }}
    >
      {displayedBreadcrumbs.map(({ text, href }, index) => {
        const isHidden = shouldTruncate && index > 0 && index < numBreadcrumbs - 1;
        return (
          <span key={text} className={isHidden ? 'sr-only' : undefined}>
            {index > 0 && <span className="mx-1">/</span>}
            {!href && <span>{text}</span>}
            {href && (
              <Link isInternal to={href} key={text}>
                {text}
              </Link>
            )}
          </span>
        );
      })}
    </span>
  );
};
