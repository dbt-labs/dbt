import type { JSX } from 'react';
import { twMerge } from 'tailwind-merge';

import { Button } from '../ui/Button';
import { Tooltip } from '../ui/Tooltip';

/**
 * Allows you to specify a custom toolbar item.
 */
export type ToolbarItem = {
  /**
   * Optional label of the button, displayed next to the icon.
   */
  label?: string;
  /**
   * The tooltip of the button, displayed on hover.
   */
  tooltip: string;
  /**
   * Called when the button is pressed.
   */
  action?: () => void;
  testId?: string;
  isDisabled?: boolean;
  className?: string;
};

interface DagToolbarProps {
  toolbarItems: ToolbarItem[];
}

export const DagToolbar = ({ toolbarItems }: DagToolbarProps): JSX.Element | null => {
  if (!toolbarItems || !toolbarItems.length) {
    return null;
  }

  const buildButton = (toolbarItem: ToolbarItem) => {
    return (
      <Tooltip
        key={toolbarItem.tooltip}
        content={toolbarItem.tooltip}
        placement="bottom"
      >
        <Button
          className={twMerge(
            'pointer-events-auto truncate whitespace-nowrap border-borderDisabled dark:border-borderMain',
            toolbarItem.className,
          )}
          text={toolbarItem.label}
          size={'icon-sm'}
          //   isDisabled={toolbarItem.isDisabled}
          onClick={toolbarItem.action}
          testId={toolbarItem.testId}
          aria-label={toolbarItem.label || toolbarItem.tooltip}
        />
      </Tooltip>
    );
  };

  return (
    <div className={`absolute right-6 top-6 flex gap-2 rounded bg-opacity-50`}>
      {toolbarItems.map((item: ToolbarItem) => buildButton(item))}
    </div>
  );
};
