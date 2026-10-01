import { FC } from 'react';
import {
  CircleCheck,
  CircleMinus,
  CircleQuestionMark,
  CircleX,
  type LucideIcon,
  Repeat,
  TriangleAlert,
} from 'lucide-react';

import { Badge, type BadgeVariant } from '../../components/ui/Badge';
import { Tooltip } from '../../components/ui/Tooltip';
import type { RunStatus } from '../typings/domain/executionInfo';
import type { FreshnessStatusValue, TestStatusValue } from '../typings/domain/status';
import { toTitleCase } from '../util/string';

type BadgeSpec = {
  variant: BadgeVariant;
  Icon?: LucideIcon;
  decorative?: boolean;
};

const RUN: Record<RunStatus, BadgeSpec> = {
  success: { variant: 'secondary', Icon: CircleCheck },
  error: { variant: 'destructive', Icon: CircleX },
  running: { variant: 'secondary' },
  queued: { variant: 'secondary' },
  skipped: { variant: 'secondary', Icon: CircleMinus },
  reused: { variant: 'secondary', Icon: Repeat },
};

const TEST: Record<TestStatusValue, BadgeSpec> = {
  pass: { variant: 'secondary', Icon: CircleCheck },
  fail: { variant: 'destructive', Icon: CircleX },
  warn: { variant: 'secondary', Icon: TriangleAlert },
  error: { variant: 'destructive', Icon: CircleX },
  skipped: { variant: 'secondary', Icon: CircleMinus },
  unknown: { variant: 'secondary', Icon: CircleQuestionMark, decorative: true },
};

const FRESH: Record<FreshnessStatusValue, BadgeSpec> = {
  pass: { variant: 'secondary', Icon: CircleCheck },
  warn: { variant: 'secondary', Icon: TriangleAlert },
  error: { variant: 'destructive', Icon: CircleX },
  outdated: { variant: 'secondary', Icon: CircleX, decorative: true },
  unconfigured: {
    variant: 'secondary',
    Icon: CircleX,
    decorative: true,
  },
  skipped: { variant: 'secondary', Icon: CircleMinus },
  unknown: { variant: 'secondary', Icon: CircleQuestionMark, decorative: true },
};

const NONE: BadgeSpec = { variant: 'secondary' };
const RUNTIME_ERROR: BadgeSpec = { variant: 'destructive', Icon: CircleX };

// Union of every vocabulary. Typed as possibly-undefined because a caller can pass a
// status outside the requested kind at runtime (e.g. a test's lastRunStatus cast
// `as RunStatus`), so a lookup miss is expected and must not be treated as always-present.
const ANY_STATUS: Record<string, BadgeSpec | undefined> = { ...FRESH, ...TEST, ...RUN };

// Read a status by own-property only, so inherited keys ("toString", "constructor",
// "__proto__", …) resolve to undefined rather than to a prototype value.
const ownSpec = (
  map: Record<string, BadgeSpec | undefined>,
  status: string,
): BadgeSpec | undefined =>
  Object.prototype.hasOwnProperty.call(map, status) ? map[status] : undefined;

// Prefer the kind's own vocabulary, fall back to any known status, then a neutral badge —
// so a status outside the declared kind renders plainly instead of crashing (META-7840).
const resolveSpec = (
  vocabulary:
    | Record<RunStatus, BadgeSpec>
    | Record<TestStatusValue, BadgeSpec>
    | Record<FreshnessStatusValue, BadgeSpec>,
  status: string,
): BadgeSpec =>
  ownSpec(vocabulary as Record<string, BadgeSpec | undefined>, status) ??
  ownSpec(ANY_STATUS, status) ??
  NONE;

type NodeStatusIconBadgeProps = (
  | { kind: 'run'; status: RunStatus }
  | { kind: 'test'; status: TestStatusValue }
  | { kind: 'freshness'; status: FreshnessStatusValue }
  | { kind: 'none' }
  | { kind: 'runtimeError' }
) & {
  className?: string;
  size?: 'small' | 'standard';
  tooltip?: string;
};

const sizeToBadgeSize = (size: 'small' | 'standard'): 'sm' | 'lg' =>
  size === 'small' ? 'sm' : 'lg';

const resolve = (
  props: NodeStatusIconBadgeProps,
): { spec: BadgeSpec; text: string } => {
  switch (props.kind) {
    case 'run':
      return { spec: resolveSpec(RUN, props.status), text: toTitleCase(props.status) };
    case 'test':
      return { spec: resolveSpec(TEST, props.status), text: toTitleCase(props.status) };
    case 'freshness':
      return {
        spec: resolveSpec(FRESH, props.status),
        text: toTitleCase(props.status),
      };
    case 'runtimeError':
      return { spec: RUNTIME_ERROR, text: 'Runtime Error' };
    case 'none':
      return { spec: NONE, text: 'Not Available' };
  }
};

export const NodeStatusIconBadge: FC<NodeStatusIconBadgeProps> = (props) => {
  const { className, size, tooltip } = props;
  const { spec, text } = resolve(props);
  const decorativeClass = spec.decorative ? '!text-fgDecorative' : '';
  const { Icon } = spec;
  return (
    <Tooltip content={tooltip}>
      <Badge
        size={sizeToBadgeSize(size ?? 'standard')}
        variant={spec.variant}
        icon={Icon && <Icon className="size-3" />}
        className={`${decorativeClass} ${className ?? ''}`.trim()}
        text={text}
      />
    </Tooltip>
  );
};
