import { FC, memo, ReactNode, useState } from 'react';

import { Badge } from '../../components/ui/Badge';
import { FloatingTabs } from '../../components/ui/FloatingTabs';

export const tabTypes = [
  'general',
  'code',
  'columns',
  'performance',
  'evaluator',
  'arguments',
  'dimensions',
  'measures',
  'queryExports',
  'tables',
  'views',
  'relationships',
  'config',
] as const;

export type TabType = (typeof tabTypes)[number];

export type TabInfo = {
  type: TabType;
  count?: number;
};

export const tabNameMap: Record<TabType, string> = {
  general: 'General',
  code: 'Code',
  columns: 'Columns',
  performance: 'Performance',
  evaluator: 'Recommendations',
  arguments: 'Arguments',
  dimensions: 'Dimensions',
  measures: 'Measures',
  queryExports: 'Query Exports',
  tables: 'Tables',
  views: 'Views',
  relationships: 'Relationships',
  config: 'Config',
};

export const tabAnnotationMap: Partial<Record<TabType, string>> = {
  relationships: 'alpha',
};

export const isTabType = (input: string | undefined | null): input is TabType => {
  return !!input && (tabTypes as readonly string[]).includes(input);
};

type TabsParams = {
  tabs: TabInfo[];
  children: (tabType: TabType) => ReactNode;
  show: boolean;
  /** Controlled active tab — when provided, caller owns state (e.g. URL sync). */
  activeTab?: TabType | null;
  /** Called when user clicks a tab. Required when `activeTab` is provided. */
  onTabChange?: (tab: TabType) => void;
  /** Rendered above the tab nav, inside the same sticky-positioned block
   *  (e.g. AssetHeader) -- so the page header and tab nav scroll together
   *  and stay pinned while only the tab content underneath scrolls. */
  stickyHeader?: ReactNode;
};

const DetailTabs: FC<TabsParams> = ({
  tabs,
  show,
  children,
  activeTab,
  onTabChange,
  stickyHeader,
}) => {
  const [internalTab, setInternalTab] = useState<TabType | null>(null);

  const isControlled = activeTab !== undefined;
  const resolvedTab: TabInfo = isControlled
    ? { type: activeTab ?? tabs[0]?.type }
    : { type: internalTab ?? tabs[0]?.type };

  const handleTabChange = (tab: TabType) => {
    if (isControlled) {
      onTabChange?.(tab);
    } else {
      setInternalTab(tab);
    }
  };

  return (
    <div
      className={`${show ? 'opacity-100' : 'opacity-0'} duration-300 motion-reduce:duration-0`}
    >
      {(stickyHeader || tabs.length > 1) && (
        <div className="sticky top-0 z-20 flex flex-col gap-6 bg-bgMain">
          {stickyHeader}
          {tabs.length > 1 && (
            <div className="overflow-x-auto">
              <FloatingTabs
                testId="resource-view-tabs"
                value={resolvedTab.type}
                onValueChange={(value) => handleTabChange(value as TabType)}
              >
                {tabs.map((tabDetails) => (
                  <FloatingTabs.Tab
                    key={tabNameMap[tabDetails.type]}
                    id={tabDetails.type}
                    count={tabDetails.count}
                    testId={`resource-view-tabs-${tabDetails.type}`}
                  >
                    <span className="flex items-center gap-1.5">
                      {tabNameMap[tabDetails.type]}
                      {tabAnnotationMap[tabDetails.type] &&
                        tabDetails.count === undefined && (
                          <Badge
                            text={tabAnnotationMap[tabDetails.type]!}
                            variant="default"
                            size="xs"
                          />
                        )}
                    </span>
                  </FloatingTabs.Tab>
                ))}
              </FloatingTabs>
            </div>
          )}
        </div>
      )}
      <>{resolvedTab.type && children(resolvedTab.type)}</>
    </div>
  );
};

export { DetailTabs };
export default memo(DetailTabs);
