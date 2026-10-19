import type { FileEntry } from '../shared';
import { buildFileTreeItems } from './fileTree';

function entry(overrides: Partial<FileEntry>): FileEntry {
  return {
    uniqueId: 'model.pkg.x',
    name: 'x',
    resourceType: 'model',
    packageName: 'pkg',
    originalFilePath: 'models/x.sql',
    patchPath: null,
    ...overrides,
  };
}

describe('buildFileTreeItems', () => {
  it('returns just the synthetic root for an empty file list', () => {
    const result = buildFileTreeItems([], 'project');

    expect(result.fileCount).toBe(0);
    expect(result.pathToUniqueId.size).toBe(0);
    expect(result.items).toHaveLength(1);
    expect(result.items[0]).toEqual({
      id: 'project',
      parent: 'root',
      data: { pathType: 'directory' },
    });
  });

  it('skips files with no package_name or original_file_path', () => {
    const result = buildFileTreeItems(
      [
        entry({ packageName: '', originalFilePath: 'models/a.sql' }),
        entry({ uniqueId: 'model.pkg.b', originalFilePath: '' }),
      ],
      'project',
    );
    expect(result.fileCount).toBe(0);
  });

  it('nests every file under the synthetic root and the package directory', () => {
    const result = buildFileTreeItems(
      [
        entry({
          uniqueId: 'model.pkg.x',
          name: 'x',
          originalFilePath: 'models/x.sql',
        }),
      ],
      'project',
    );

    const root = result.items.find((i) => i.id === 'project');
    const pkg = result.items.find((i) => i.id === 'project/pkg');
    const modelsDir = result.items.find((i) => i.id === 'project/pkg/models');
    const leaf = result.items.find((i) => i.id === 'project/pkg/models/x.sql');

    expect(root?.parent).toBe('root');
    expect(pkg?.parent).toBe('project');
    expect(pkg?.data.pathType).toBe('directory');
    expect(modelsDir?.parent).toBe('project/pkg');
    expect(modelsDir?.data.pathType).toBe('directory');
    expect(leaf?.parent).toBe('project/pkg/models');
    expect(leaf?.data.pathType).toBe('file');
    expect(result.pathToUniqueId.get('project/pkg/models/x.sql')).toBe('model.pkg.x');
    expect(result.fileCount).toBe(1);
  });

  it('attaches a resource-type icon override to each leaf', () => {
    const result = buildFileTreeItems(
      [entry({ resourceType: 'model', originalFilePath: 'models/x.sql' })],
      'project',
    );
    const leaf = result.items.find((i) => i.id === 'project/pkg/models/x.sql');
    expect(leaf?.data.iconOverride?.label).toBe('model');
    expect(leaf?.data.iconOverride?.icon).toBeDefined();
  });

  it('deduplicates folder items shared across multiple files', () => {
    const result = buildFileTreeItems(
      [
        entry({
          uniqueId: 'model.pkg.a',
          name: 'a',
          originalFilePath: 'models/staging/a.sql',
        }),
        entry({
          uniqueId: 'model.pkg.b',
          name: 'b',
          originalFilePath: 'models/staging/b.sql',
        }),
      ],
      'project',
    );

    const stagingDirs = result.items.filter(
      (i) => i.id === 'project/pkg/models/staging',
    );
    expect(stagingDirs).toHaveLength(1);
    expect(result.fileCount).toBe(2);
    expect(result.pathToUniqueId.size).toBe(2);
  });

  it('groups multiple resources sharing a YAML file as siblings under a YAML folder', () => {
    const result = buildFileTreeItems(
      [
        entry({
          uniqueId: 'test.pkg.t_one',
          name: 't_one',
          resourceType: 'test',
          originalFilePath: 'models/_models.yml',
        }),
        entry({
          uniqueId: 'test.pkg.t_two',
          name: 't_two',
          resourceType: 'test',
          originalFilePath: 'models/_models.yml',
        }),
      ],
      'project',
    );

    const yamlDir = result.items.find((i) => i.id === 'project/pkg/models/_models.yml');
    expect(yamlDir?.data.pathType).toBe('directory');
    // No icon override -- renders as a plain folder like any other node with
    // children, not a resource/file-specific icon (matches dbt Platform prod).
    expect(yamlDir?.data.iconOverride).toBeUndefined();

    expect(result.pathToUniqueId.get('project/pkg/models/_models.yml/t_one')).toBe(
      'test.pkg.t_one',
    );
    expect(result.pathToUniqueId.get('project/pkg/models/_models.yml/t_two')).toBe(
      'test.pkg.t_two',
    );
    expect(result.fileCount).toBe(2);
  });

  it('treats .yaml the same as .yml', () => {
    const result = buildFileTreeItems(
      [
        entry({
          uniqueId: 'test.pkg.t_one',
          name: 't_one',
          resourceType: 'test',
          originalFilePath: 'models/_models.yaml',
        }),
      ],
      'project',
    );
    const yamlDir = result.items.find(
      (i) => i.id === 'project/pkg/models/_models.yaml',
    );
    expect(yamlDir?.data.pathType).toBe('directory');
    expect(yamlDir?.data.iconOverride).toBeUndefined();
  });

  it('keeps files from different packages in separate package roots', () => {
    const result = buildFileTreeItems(
      [
        entry({
          uniqueId: 'model.a.x',
          packageName: 'a',
          originalFilePath: 'models/x.sql',
        }),
        entry({
          uniqueId: 'model.b.x',
          packageName: 'b',
          originalFilePath: 'models/x.sql',
        }),
      ],
      'project',
    );

    expect(result.items.find((i) => i.id === 'project/a')).toBeDefined();
    expect(result.items.find((i) => i.id === 'project/b')).toBeDefined();
    expect(result.pathToUniqueId.get('project/a/models/x.sql')).toBe('model.a.x');
    expect(result.pathToUniqueId.get('project/b/models/x.sql')).toBe('model.b.x');
    expect(result.fileCount).toBe(2);
  });
});
