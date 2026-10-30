import {
  DropdownButton,
  type DropdownOption,
} from '../../components/ui/DropdownButton';

export interface FilterDropdownProps {
  options: DropdownOption[];
  defaultOption: DropdownOption;
  name: string;
  onChange: (option: DropdownOption) => void;
  isDisabled?: boolean;
  className?: string;
  listClassName?: string;
  trackingId?: string;
}

export function FilterDropdown({
  options,
  defaultOption,
  name,
  onChange,
  isDisabled,
  className,
  listClassName,
  trackingId,
}: FilterDropdownProps) {
  return (
    <DropdownButton
      options={options}
      name={name}
      defaultOption={defaultOption}
      isDisabled={isDisabled}
      className={className}
      listClassName={listClassName}
      trackingId={trackingId}
      onChange={onChange}
    />
  );
}
