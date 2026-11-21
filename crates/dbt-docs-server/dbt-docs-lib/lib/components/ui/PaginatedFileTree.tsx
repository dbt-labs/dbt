import { type MouseEvent, type ReactNode, useMemo } from 'react';

import { FileTreeNode } from './FileTreeNode';

export type FileTreeItemType = {
  id: string;
  parent: string;
  data: {
    pathType: 'file' | 'directory';
    name?: string;
    iconOverride?: { icon: ReactNode; label?: string };
    info?: { text?: string };
  };
};

export type OnFileSelect = (
  relativePath: string,
  event: MouseEvent<HTMLButtonElement>,
  item: FileTreeItemType,
) => void;
export type OnFolderSelect = (
  relativePath: string,
  event: MouseEvent<HTMLButtonElement>,
  isExpandButtonClick?: boolean,
) => void;
export type SetOpenDirectories = (
  updater: (directories: string[] | undefined) => string[] | undefined,
  event: MouseEvent<HTMLButtonElement>,
) => void;

export interface PaginatedFileTreeProps {
  items: FileTreeItemType[];
  rootNodeName: string;
  openDirectories: string[];
  setOpenDirectories: SetOpenDirectories;
  onFileSelect?: OnFileSelect;
  onFolderSelect?: OnFolderSelect;
  selectedFile?: string;
  selectedFolder?: string;
  enableCloseFolderOnSecondClick?: boolean;
  onSort?: (a: string, b: string) => number;
  maxHeight?: number;
}

export function displayName(item: FileTreeItemType): string {
  return item.data.name ?? item.id.split('/').pop() ?? item.id;
}

export function PaginatedFileTree({
  items,
  rootNodeName,
  openDirectories,
  setOpenDirectories,
  onFileSelect,
  onFolderSelect,
  selectedFile,
  selectedFolder,
  enableCloseFolderOnSecondClick,
  onSort,
  maxHeight,
}: PaginatedFileTreeProps) {
  const childrenByParent = useMemo(() => {
    const map = new Map<string, FileTreeItemType[]>();
    for (const item of items) {
      const arr = map.get(item.parent);
      if (arr) arr.push(item);
      else map.set(item.parent, [item]);
    }
    const compare = onSort
      ? (a: FileTreeItemType, b: FileTreeItemType) => onSort(a.id, b.id)
      : (a: FileTreeItemType, b: FileTreeItemType) => {
          if (a.data.pathType !== b.data.pathType) {
            return a.data.pathType === 'directory' ? -1 : 1;
          }
          return displayName(a).localeCompare(displayName(b));
        };
    for (const arr of map.values()) arr.sort(compare);
    return map;
  }, [items, onSort]);

  const rootItems = childrenByParent.get(rootNodeName) ?? [];

  return (
    <div
      role="tree"
      aria-label="File tree"
      className="overflow-y-auto"
      style={{ maxHeight }}
    >
      {rootItems.map((item) => (
        <FileTreeNode
          key={item.id}
          item={item}
          depth={0}
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
    </div>
  );
}
