import { type ReactNode } from 'react';
import { CircleQuestionMark } from 'lucide-react';

import { SelectorLink } from './SelectorLink';

export interface LineageQuickLink {
  selector: string;
  label: string;
}

interface Props {
  description: ReactNode;
  quickLinks?: LineageQuickLink[];
}

export function LineageEmptyState({ description, quickLinks }: Props) {
  return (
    <div className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 space-y-2 whitespace-nowrap rounded-md border border-borderMuted bg-bgMain p-6 text-center text-sm">
      <div>
        <CircleQuestionMark className="size-5 text-fgBrand" />
      </div>
      <div className="text-lg text-fgMain">What are you looking for?</div>
      <div className="text-fgDecorative">{description}</div>
      {quickLinks && quickLinks.length > 0 && (
        <div>
          {quickLinks.map(({ selector, label }) => (
            <span key={label} className="px-1">
              <SelectorLink selector={selector} label={label} />
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
