import { createElement, useState } from 'react';

import { FloatingTabs } from '../../components/ui/FloatingTabs';
import { getResourceType, iconForType } from '../../lib/resourceType';

export type RelationshipItem = {
  uniqueId: string;
  name: string;
  resourceType: string;
};

export type AssetRelationshipsProps = {
  dependsOn: RelationshipItem[];
  referencedBy: RelationshipItem[];
  onSelect?: (uniqueId: string) => void;
};

type SubTab = 'dependsOn' | 'referencedBy';

function RelationshipList({
  items,
  onSelect,
}: {
  items: RelationshipItem[];
  onSelect?: (uniqueId: string) => void;
}) {
  if (items.length === 0) {
    return <p className="mt-2 italic text-fgDecorative">No relationships found.</p>;
  }

  return (
    <div className="space-y-2">
      {items.map((item) => {
        const resourceType = getResourceType(item.resourceType, 'unknown');
        const inner = (
          <>
            <span className="flex-0 m-1 h-4 w-4">
              {createElement(iconForType(resourceType), {
                className: 'align-top text-current',
                size: 16,
              })}
            </span>
            <span className="align-middle">{item.name}</span>
          </>
        );

        return (
          <div key={item.uniqueId} className="flex">
            {onSelect ? (
              <button
                type="button"
                className="flex underline"
                onClick={() => onSelect(item.uniqueId)}
              >
                {inner}
              </button>
            ) : (
              <div className="flex">{inner}</div>
            )}
          </div>
        );
      })}
    </div>
  );
}

export function AssetRelationships({
  dependsOn,
  referencedBy,
  onSelect,
}: AssetRelationshipsProps) {
  const initialTab: SubTab = dependsOn.length > 0 ? 'dependsOn' : 'referencedBy';
  const [activeTab, setActiveTab] = useState<SubTab>(initialTab);

  const noRelationships = dependsOn.length === 0 && referencedBy.length === 0;

  return (
    <div className="space-y-6">
      <div className="overflow-x-auto">
        {noRelationships ? (
          <p className="italic text-fgDecorative">No relationships found.</p>
        ) : (
          <FloatingTabs
            value={activeTab}
            onValueChange={(value) => setActiveTab(value as SubTab)}
          >
            {dependsOn.length > 0 && (
              <FloatingTabs.Tab id="dependsOn">Depends on</FloatingTabs.Tab>
            )}
            {referencedBy.length > 0 && (
              <FloatingTabs.Tab id="referencedBy">Referenced by</FloatingTabs.Tab>
            )}
          </FloatingTabs>
        )}
      </div>
      {!noRelationships && (
        <RelationshipList
          items={activeTab === 'dependsOn' ? dependsOn : referencedBy}
          onSelect={onSelect}
        />
      )}
    </div>
  );
}
