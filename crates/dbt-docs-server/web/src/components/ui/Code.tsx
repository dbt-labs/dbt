import { type ReactNode } from 'react';

import { cn } from '../../lib/utils';

export interface CodeProps {
  children?: ReactNode;
  className?: string;
}

export function Code({ children, className }: CodeProps) {
  return (
    <code
      className={cn(
        'relative rounded bg-bgMainActive px-[0.3rem] py-[0.2rem] font-mono text-sm',
        className,
      )}
    >
      {children}
    </code>
  );
}
