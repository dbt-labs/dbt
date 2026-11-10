import * as SelectPrimitive from '@radix-ui/react-select';
import { ChevronDown } from 'lucide-react';

import { cn } from '../../lib/utils';

export interface DropdownOption {
  label: string;
  value: string | number;
}

export interface DropdownButtonProps {
  name?: string;
  defaultOption?: DropdownOption;
  options: DropdownOption[];
  onChange: (selected: DropdownOption) => void;
  isDisabled?: boolean;
  className?: string;
  listClassName?: string;
  trackingId?: string;
  testId?: string;
}

export function DropdownButton({
  name,
  defaultOption,
  options,
  onChange,
  isDisabled,
  className,
  listClassName,
  trackingId,
  testId,
}: DropdownButtonProps) {
  const handleChange = (value: string) => {
    const selected = options.find((option) => String(option.value) === value);
    if (selected) onChange(selected);
  };

  return (
    <SelectPrimitive.Root
      defaultValue={defaultOption ? String(defaultOption.value) : undefined}
      onValueChange={handleChange}
      disabled={isDisabled}
    >
      <SelectPrimitive.Trigger
        className={cn(
          'inline-flex items-center gap-1.5 rounded-md border border-borderMain bg-bgMain px-3 py-1.5 text-sm text-fgMain hover:bg-bgMainHover disabled:pointer-events-none disabled:opacity-50',
          className,
        )}
        data-testid={testId}
        data-trackingid={trackingId}
      >
        {name && <span className="text-fgDecorative">{name}</span>}
        <SelectPrimitive.Value />
        <SelectPrimitive.Icon>
          <ChevronDown className="size-3" />
        </SelectPrimitive.Icon>
      </SelectPrimitive.Trigger>
      <SelectPrimitive.Portal>
        <SelectPrimitive.Content
          position="popper"
          sideOffset={4}
          className={cn(
            'z-50 overflow-hidden rounded-md border border-borderMain bg-bgMain shadow-md',
            listClassName,
          )}
        >
          <SelectPrimitive.Viewport className="p-1">
            {options.map((option) => (
              <SelectPrimitive.Item
                key={String(option.value)}
                value={String(option.value)}
                className="relative flex cursor-pointer select-none items-center rounded px-2 py-1.5 text-sm text-fgMain outline-none data-[highlighted]:bg-bgMainHover"
              >
                <SelectPrimitive.ItemText>{option.label}</SelectPrimitive.ItemText>
              </SelectPrimitive.Item>
            ))}
          </SelectPrimitive.Viewport>
        </SelectPrimitive.Content>
      </SelectPrimitive.Portal>
    </SelectPrimitive.Root>
  );
}
