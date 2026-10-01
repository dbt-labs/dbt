import type { DropdownOption } from '../components/ui/DropdownButton';
import type { FacetValue } from '../shared';

/** The leading "All" (no-filter) option shared by every facet dropdown. */
export const ALL_FACET_OPTION: DropdownOption = { label: 'All', value: '' };

/** Build `FilterDropdown` options from facet values, prefixed with the "All"
 *  option. Labels carry the count when present (`value (count)`). `formatValue`
 *  optionally transforms the displayed value (e.g. capitalize). */
export function facetOptions(
  values: FacetValue[] | undefined,
  formatValue: (value: string) => string = (v) => v,
): DropdownOption[] {
  return [
    ALL_FACET_OPTION,
    ...(values ?? []).map((v) => {
      const label = formatValue(v.value);
      return {
        label: v.count != null ? `${label} (${v.count})` : label,
        value: v.value,
      };
    }),
  ];
}

/** The option matching `value`, falling back to the "All" option. */
export function selectedFacetOption(
  options: DropdownOption[],
  value: string,
): DropdownOption {
  return options.find((o) => o.value === value) ?? ALL_FACET_OPTION;
}
