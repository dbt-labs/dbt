import { FC } from 'react';
import {
  CircleCheck,
  CircleMinus,
  CircleX,
  type LucideIcon,
  Repeat,
  TriangleAlert,
} from 'lucide-react';
import { twMerge } from 'tailwind-merge';

import type { TestStatus } from '../util/testStatus';

interface TestStatusIconProps {
  status: TestStatus;
  className?: string;
}

type TestStatusIconType = { Icon: LucideIcon; className: string };

const getIconConfig = (status: TestStatus): TestStatusIconType | null => {
  switch (status) {
    case 'error':
      return { Icon: CircleX, className: 'text-fgDanger' };
    case 'fail':
      return { Icon: CircleX, className: 'text-fgDanger' };
    case 'pass':
      return { Icon: CircleCheck, className: 'text-fgSuccess' };
    case 'reused':
      return { Icon: Repeat, className: 'text-fgVizTeal' };
    case 'skipped':
      return { Icon: CircleMinus, className: 'text-fgDecorative' };
    case 'warn':
      return { Icon: TriangleAlert, className: 'text-fgWarning' };
    default:
      return null;
  }
};

export const TestStatusIcon: FC<TestStatusIconProps> = ({ status, className }) => {
  const statusConfig = getIconConfig(status);
  if (!statusConfig) return null;
  const { Icon } = statusConfig;
  return (
    <Icon
      aria-label={status}
      className={twMerge(statusConfig.className, 'size-4', className)}
    />
  );
};
