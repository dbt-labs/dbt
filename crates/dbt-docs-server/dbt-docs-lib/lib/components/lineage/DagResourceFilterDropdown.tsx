import { useState } from 'react';
import { ChevronDown, ListFilter } from 'lucide-react';

import { useLineageStore } from '../../stores/lineageStore';
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from '../ui/DropdownMenu';
import {
  alwaysVisibleResourceTypes,
  DEFAULT_ADDITIONAL_RESOURCE_TYPES,
  DEFAULT_RESOURCE_TYPE_FILTER_LABELS,
  DEFAULT_VISIBLE_RESOURCE_TYPES,
  ResourceType,
} from './ResourceTypes';

interface DagResourceFilterDropdownProps {
  // resource types shown on initial load and always displayed in the dropdown
  defaultVisibleResourceTypes?: Set<ResourceType>;
  // resource types hidden on initial load and hidden below "see more" in the dropdown
  additionalResourceTypes?: Set<ResourceType>;
  // string labels used in dropdown for resource types
  resourceTypeLabels?: Record<ResourceType, string>;
}

export function DagResourceFilterDropdown({
  defaultVisibleResourceTypes = DEFAULT_VISIBLE_RESOURCE_TYPES,
  additionalResourceTypes = DEFAULT_ADDITIONAL_RESOURCE_TYPES,
  resourceTypeLabels = DEFAULT_RESOURCE_TYPE_FILTER_LABELS,
}: DagResourceFilterDropdownProps) {
  const [showMore, setShowMore] = useState(false);
  const rootUniqueId = useLineageStore((s) => s.rootUniqueId);
  const activeResourceFilters = useLineageStore((s) => s.visibleResourceTypes);
  const setResourceFilters = useLineageStore((s) => s.setResourceTypesVisible);
  // Models and the node's own type -- if you're viewing a test, tests -- can't be
  // unchecked. The store keeps them in `visibleResourceTypes` whatever it's handed,
  // so every checkbox, these included, just reads it.
  const alwaysVisible = alwaysVisibleResourceTypes(rootUniqueId);

  const isChecked = (value: ResourceType) => activeResourceFilters.has(value);

  const toggleResourceFilter = (resource: ResourceType, enabled: boolean) => {
    const selectedResource = new Set([resource]);
    setResourceFilters(
      enabled
        ? activeResourceFilters.union(selectedResource)
        : activeResourceFilters.difference(selectedResource),
    );
  };

  // clicking reset all leaves only what can't be unchecked: models and the node's own
  // resource type
  const resetResourceFilters = () => setResourceFilters(new Set());

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label="Filter"
          className="flex h-9 items-center gap-1 rounded-md border border-borderMain bg-bgMain px-2.5 text-sm text-fgMain hover:bg-bgMainHover"
        >
          <ListFilter className="size-3.5" />
          <ChevronDown className="size-3.5 text-fgDecorative" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-56">
        <div className="px-2 pb-1.5 pt-1 text-xs text-fgAlt">Filter by resource</div>
        {[...defaultVisibleResourceTypes].map((value) => (
          <DropdownMenuCheckboxItem
            key={value}
            checked={isChecked(value)}
            onCheckedChange={(checked) => toggleResourceFilter(value, checked)}
            disabled={alwaysVisible.has(value)}
          >
            {resourceTypeLabels[value]}
          </DropdownMenuCheckboxItem>
        ))}
        {showMore &&
          [...additionalResourceTypes].map((value) => (
            <DropdownMenuCheckboxItem
              key={value}
              checked={isChecked(value)}
              onCheckedChange={(checked) => toggleResourceFilter(value, checked)}
              disabled={alwaysVisible.has(value)}
            >
              {resourceTypeLabels[value]}
            </DropdownMenuCheckboxItem>
          ))}
        <div className="my-1 border-t border-borderMuted" />
        <div className="flex items-center justify-between px-2 py-1">
          <button
            type="button"
            className="text-sm text-fgBrand underline hover:no-underline"
            onClick={() => setShowMore((prev) => !prev)}
          >
            {showMore ? 'See less' : 'See more'}
          </button>
          <button
            type="button"
            className="text-sm text-fgBrand underline hover:no-underline"
            onClick={() => resetResourceFilters()}
          >
            Deselect all
          </button>
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
