import { type ReactNode } from 'react';
import { cva, type VariantProps } from 'class-variance-authority';

import { cn } from '../../lib/utils';

const notificationBannerVariants = cva('rounded-md border p-3 text-sm', {
  variants: {
    type: {
      error: 'border-borderDanger bg-bgDangerMuted text-fgDanger',
      warning: 'border-borderWarning bg-bgWarningMuted text-fgWarning',
      info: 'border-borderInfo bg-bgInfoMuted text-fgInfo',
    },
  },
  defaultVariants: {
    type: 'info',
  },
});

export interface Notification {
  /** The header to display in the banner */
  header: ReactNode;
  /** An id for the banner */
  id: string | number;
  /** The banner type */
  type?: NonNullable<VariantProps<typeof notificationBannerVariants>['type']>;
}

export interface NotificationBannerProps {
  notification: Notification;
  className?: string;
}

export function NotificationBanner({
  notification,
  className,
}: NotificationBannerProps) {
  return (
    <div
      role="alert"
      className={cn(notificationBannerVariants({ type: notification.type }), className)}
    >
      {notification.header}
    </div>
  );
}
