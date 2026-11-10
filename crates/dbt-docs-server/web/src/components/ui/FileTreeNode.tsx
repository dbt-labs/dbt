import { type MouseEvent } from 'react';
import { ChevronRight, FileText, Folder } from 'lucide-react';

import { cn } from '../../lib/utils';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from './Collapsible';
import {
  displayName,
  type FileTreeItemType,
  type OnFileSelect,
  type OnFolderSelect,
  type SetOpenDirectories,
} from './PaginatedFileTree';

function toggle(directories: string[] | undefined, id: string, shouldClose: boolean) {
  const set = new Set(directories ?? []);
  if (shouldClose) set.delete(id);
  else set.add(id);
  return [...set];
}

interface FileTreeNodeProps {
  item: FileTreeItemType;
  depth: number;
  childrenByParent: Map<string, FileTreeItemType[]>;
  openDirectories: string[];
  setOpenDirectories: SetOpenDirectories;
  onFileSelect?: OnFileSelect;
  onFolderSelect?: OnFolderSelect;
  selectedFile?: string;
  selectedFolder?: string;
  enableCloseFolderOnSecondClick?: boolean;
}

export function FileTreeNode({
  item,
  depth,
  childrenByParent,
  openDirectories,
  setOpenDirectories,
  onFileSelect,
  onFolderSelect,
  selectedFile,
  selectedFolder,
  enableCloseFolderOnSecondClick,
}: FileTreeNodeProps) {
  const name = displayName(item);
  const paddingLeft = depth * 16;
  const infoText = item.data.info?.text;

  if (item.data.pathType === 'file') {
    const isSelected = selectedFile === item.id;
    return (
      <button
        type="button"
        style={{ paddingLeft }}
        className={cn(
          'flex w-full items-center gap-1.5 rounded px-2 py-1 text-left text-sm text-fgMain hover:bg-bgMainHover',
          isSelected && 'bg-bgMainActive font-medium',
        )}
        onClick={(event) => onFileSelect?.(item.id, event, item)}
      >
        {item.data.iconOverride?.icon ?? <FileText className="size-3 shrink-0" />}
        <span className="truncate">{name}</span>
        {infoText && (
          <span className="ml-auto shrink-0 text-xs text-fgDecorative">{infoText}</span>
        )}
      </button>
    );
  }

  const isOpen = openDirectories.includes(item.id);
  const isSelectedFolder = selectedFolder === item.id;
  const children = childrenByParent.get(item.id) ?? [];

  const handleChevronClick = (event: MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation();
    setOpenDirectories((dirs) => toggle(dirs, item.id, isOpen), event);
    onFolderSelect?.(item.id, event, true);
  };

  const handleRowClick = (event: MouseEvent<HTMLButtonElement>) => {
    const shouldClose = !!enableCloseFolderOnSecondClick && isOpen && isSelectedFolder;
    setOpenDirectories((dirs) => toggle(dirs, item.id, shouldClose), event);
    onFolderSelect?.(item.id, event, false);
  };

  return (
    <Collapsible open={isOpen}>
      <div
        style={{ paddingLeft }}
        className={cn(
          'flex w-full items-center gap-1.5 rounded px-2 py-1 text-sm text-fgMain hover:bg-bgMainHover',
          isSelectedFolder && 'bg-bgMainActive font-medium',
        )}
      >
        <CollapsibleTrigger asChild onClick={handleChevronClick}>
          <button
            type="button"
            className="shrink-0"
            aria-label={isOpen ? 'Collapse' : 'Expand'}
          >
            <ChevronRight
              className={cn('size-3 transition-transform', isOpen && 'rotate-90')}
            />
          </button>
        </CollapsibleTrigger>
        <button
          type="button"
          className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
          onClick={handleRowClick}
        >
          {item.data.iconOverride?.icon ?? <Folder className="size-3 shrink-0" />}
          <span className="truncate">{name}</span>
        </button>
        {infoText && (
          <span className="ml-auto shrink-0 text-xs text-fgDecorative">{infoText}</span>
        )}
      </div>
      <CollapsibleContent>
        {children.map((child) => (
          <FileTreeNode
            key={child.id}
            item={child}
            depth={depth + 1}
            childrenByParent={childrenByParent}
            openDirectories={openDirectories}
            setOpenDirectories={setOpenDirectories}
            onFileSelect={onFileSelect}
            onFolderSelect={onFolderSelect}
            selectedFile={selectedFile}
            selectedFolder={selectedFolder}
            enableCloseFolderOnSecondClick={enableCloseFolderOnSecondClick}
          />
        ))}
      </CollapsibleContent>
    </Collapsible>
  );
}
