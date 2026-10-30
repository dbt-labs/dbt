import { ClipboardCheck, FileText, type LucideIcon } from 'lucide-react';

import type { FileTreeItemType } from '../components/ui/PaginatedFileTree';
import type { FileEntry } from '../shared';
import { RESOURCE_TYPE_ICON } from './resourceType';

const EXTRA_ICON: Record<string, LucideIcon> = {
  unit_test: ClipboardCheck,
  doc: FileText,
};

export function iconForResourceType(resourceType: string): LucideIcon {
  return RESOURCE_TYPE_ICON[resourceType] ?? EXTRA_ICON[resourceType] ?? FileText;
}

const YAML_PATH_RE = /\.ya?ml$/i;

export interface BuiltFileTree {
  items: FileTreeItemType[];
  /** Tree-id (full path) → resource unique_id. */
  pathToUniqueId: Map<string, string>;
  /** Total file rows (excludes folders). Used for the locate-pane summary. */
  fileCount: number;
}

/** Turn the flat FileEntry list into the shape sourdough's FileTree expects:
 *  one item per path segment, files marked `pathType: 'file'` and folders
 *  marked `pathType: 'directory'`. Each item's `id` is its full path so the
 *  component can derive depth + parent from path-splitting alone.
 *
 *  Everything is nested under a single synthetic root folder named `rootName`
 *  so sourdough's treeWalker (which has a bug when fed multiple roots) sees
 *  exactly one root child of `'root'`. */
export function buildFileTreeItems(
  files: FileEntry[],
  rootName: string,
): BuiltFileTree {
  const itemsByPath = new Map<string, FileTreeItemType>();
  const pathToUniqueId = new Map<string, string>();
  let fileCount = 0;

  itemsByPath.set(rootName, {
    id: rootName,
    parent: 'root',
    data: { pathType: 'directory' },
  });

  for (const file of files) {
    const segments = pathSegments(file);
    if (segments.length === 0) continue;
    fileCount += 1;

    for (let i = 0; i < segments.length; i += 1) {
      const path = `${rootName}/${segments.slice(0, i + 1).join('/')}`;
      const isLeaf = i === segments.length - 1;
      if (itemsByPath.has(path)) continue;

      const parent =
        i === 0 ? rootName : `${rootName}/${segments.slice(0, i).join('/')}`;

      if (isLeaf) {
        const FileIcon = iconForResourceType(file.resourceType);
        itemsByPath.set(path, {
          id: path,
          parent,
          data: {
            pathType: 'file',
            iconOverride: {
              icon: <FileIcon className="size-3 shrink-0" />,
              label: file.resourceType,
            },
          },
        });
        pathToUniqueId.set(path, file.uniqueId);
      } else {
        // Any node with children renders as a plain folder, regardless of
        // what it actually is on disk -- a multi-resource yaml file included
        // its own icon here once, but that read as "this is a file" even
        // though it expands like a directory. Matches dbt Platform prod:
        // resource-specific icons only apply at leaf level (nothing nested
        // underneath); anything with children is just a folder.
        itemsByPath.set(path, {
          id: path,
          parent,
          data: { pathType: 'directory' },
        });
      }
    }
  }

  return {
    items: Array.from(itemsByPath.values()),
    pathToUniqueId,
    fileCount,
  };
}

/** Files whose `original_file_path` is a YAML doc (e.g. `models/_models.yml`)
 *  are turned into folders, with one leaf per resource defined inside that
 *  file. The leaf segment is the resource's own `name`, so two tests sharing
 *  `_models.yml` no longer collide on path-dedup. */
function pathSegments(file: FileEntry): string[] {
  if (!file.packageName || !file.originalFilePath) return [];
  const parts = file.originalFilePath.split('/').filter(Boolean);
  if (parts.length === 0) return [];
  const last = parts[parts.length - 1];
  if (YAML_PATH_RE.test(last) && file.name) {
    return [file.packageName, ...parts, file.name];
  }
  return [file.packageName, ...parts];
}
