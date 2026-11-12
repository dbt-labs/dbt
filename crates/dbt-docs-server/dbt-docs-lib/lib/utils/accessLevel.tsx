import { Ban, Globe, Lock, ShieldCheck, ShieldOff } from 'lucide-react';

import { Tooltip, type TooltipProps } from '../components/ui/Tooltip';

/** dbt's model-access modifiers (`private`/`protected`/`public`), plus two
 *  presentation-only states for when access can't be determined. */
export const ACCESS_TYPES = [
  'private',
  'protected',
  'public',
  'no_access',
  'no_permission',
] as const;

export type AccessType = (typeof ACCESS_TYPES)[number];

export function getAccessType<TDefaultValue extends undefined | AccessType>(
  accessType: string | undefined,
  defaultValue?: TDefaultValue,
): AccessType | TDefaultValue {
  return (ACCESS_TYPES as readonly string[]).includes(accessType ?? '')
    ? (accessType as AccessType)
    : (defaultValue as TDefaultValue);
}

const ACCESS_TYPE_ICON: Record<AccessType, typeof Lock> = {
  private: Lock,
  protected: ShieldCheck,
  public: Globe,
  no_access: Ban,
  no_permission: ShieldOff,
};

const ACCESS_TYPE_LABEL: Record<AccessType, string> = {
  private: 'Private',
  protected: 'Protected',
  public: 'Public',
  no_access: 'No access',
  no_permission: 'No permission',
};

export interface AccessLevelIconProps {
  access: AccessType | undefined;
  tooltipConfig?: Omit<TooltipProps, 'content' | 'children'>;
  iconClassName?: string;
}

export function AccessLevelIcon({
  access,
  tooltipConfig,
  iconClassName,
}: AccessLevelIconProps) {
  if (!access) return null;
  const Icon = ACCESS_TYPE_ICON[access];
  const icon = (
    <Icon className={iconClassName} aria-label={ACCESS_TYPE_LABEL[access]} />
  );
  return tooltipConfig ? (
    <Tooltip {...tooltipConfig} content={ACCESS_TYPE_LABEL[access]}>
      {icon}
    </Tooltip>
  ) : (
    icon
  );
}
