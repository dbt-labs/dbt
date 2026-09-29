import { type ComponentProps } from 'react';
import { twMerge } from 'tailwind-merge';

export function Table({ className, ...props }: ComponentProps<'table'>) {
  return (
    <div className="relative w-full overflow-x-auto">
      <table className={twMerge('w-full text-sm', className)} {...props} />
    </div>
  );
}

export function TableHeader({ className, ...props }: ComponentProps<'thead'>) {
  return <thead className={twMerge('[&_tr]:border-b', className)} {...props} />;
}

export function TableBody({ className, ...props }: ComponentProps<'tbody'>) {
  return (
    <tbody className={twMerge('[&_tr:last-child]:border-0', className)} {...props} />
  );
}

export function TableRow({ className, ...props }: ComponentProps<'tr'>) {
  return (
    <tr
      className={twMerge(
        'border-b border-borderMuted transition-colors hover:bg-bgMainHover',
        className,
      )}
      {...props}
    />
  );
}

export function TableHead({ className, ...props }: ComponentProps<'th'>) {
  return (
    <th
      className={twMerge(
        'h-10 whitespace-nowrap px-3 text-left align-middle font-medium text-fgAlt',
        className,
      )}
      {...props}
    />
  );
}

export function TableCell({ className, ...props }: ComponentProps<'td'>) {
  return (
    <td
      className={twMerge('whitespace-nowrap px-3 py-2 align-middle', className)}
      {...props}
    />
  );
}
