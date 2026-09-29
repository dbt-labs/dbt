import { type ComponentProps, type ComponentType } from 'react';
import * as DropdownMenuPrimitive from '@radix-ui/react-dropdown-menu';
import { Check } from 'lucide-react';

import { cn } from '../../lib/utils';

export const DropdownMenu = DropdownMenuPrimitive.Root;
export const DropdownMenuTrigger = DropdownMenuPrimitive.Trigger;
export const DropdownMenuGroup = DropdownMenuPrimitive.Group;
export const DropdownMenuRadioGroup = DropdownMenuPrimitive.RadioGroup;

type DropdownMenuContentProps = ComponentProps<typeof DropdownMenuPrimitive.Content> & {
  // Present at runtime on Radix's underlying Menu.Content, but not surfaced
  // through DropdownMenuContentProps' generated type in this Radix version.
  onOpenAutoFocus?: (event: Event) => void;
};

export function DropdownMenuContent({
  className,
  sideOffset = 4,
  onOpenAutoFocus,
  ...props
}: DropdownMenuContentProps) {
  const ContentPrimitive = DropdownMenuPrimitive.Content as ComponentType<
    ComponentProps<typeof DropdownMenuPrimitive.Content> & {
      onOpenAutoFocus?: (event: Event) => void;
    }
  >;

  return (
    <DropdownMenuPrimitive.Portal>
      <ContentPrimitive
        sideOffset={sideOffset}
        // Keep keyboard focus on the trigger (our input) instead of Radix's
        // default "focus the first item" behavior, so a user can keep typing
        // a custom value while the suggestion list is open.
        onOpenAutoFocus={onOpenAutoFocus ?? ((event) => event.preventDefault())}
        className={cn(
          'z-50 min-w-32 overflow-hidden rounded-md border border-borderMain bg-bgMain p-1 shadow-md',
          className,
        )}
        {...props}
      />
    </DropdownMenuPrimitive.Portal>
  );
}

export function DropdownMenuItem({
  className,
  ...props
}: ComponentProps<typeof DropdownMenuPrimitive.Item>) {
  return (
    <DropdownMenuPrimitive.Item
      className={cn(
        'flex cursor-pointer select-none items-center justify-between rounded px-2 py-1.5 text-sm text-fgMain outline-none data-[highlighted]:bg-bgMainHover',
        className,
      )}
      {...props}
    />
  );
}

export function DropdownMenuRadioItem({
  className,
  children,
  ...props
}: ComponentProps<typeof DropdownMenuPrimitive.RadioItem>) {
  return (
    <DropdownMenuPrimitive.RadioItem
      className={cn(
        'flex cursor-pointer select-none items-center justify-between rounded px-2 py-1.5 text-sm text-fgMain outline-none data-[highlighted]:bg-bgMainHover',
        className,
      )}
      {...props}
    >
      {children}
      <DropdownMenuPrimitive.ItemIndicator>
        <Check className="size-3.5 text-fgBrand" />
      </DropdownMenuPrimitive.ItemIndicator>
    </DropdownMenuPrimitive.RadioItem>
  );
}

export function DropdownMenuSeparator({
  className,
  ...props
}: ComponentProps<typeof DropdownMenuPrimitive.Separator>) {
  return (
    <DropdownMenuPrimitive.Separator
      className={cn('my-1 border-t border-borderMuted', className)}
      {...props}
    />
  );
}
