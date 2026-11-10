import { viewFromPath } from './viewFromPath';

describe('viewFromPath', () => {
  it('returns home for the root path', () => {
    expect(viewFromPath('/')).toEqual({ kind: 'home' });
  });

  it('returns home for an unknown path', () => {
    expect(viewFromPath('/some/unknown/path')).toEqual({ kind: 'home' });
  });

  it('parses /search/ as a list view with no type', () => {
    expect(viewFromPath('/search/')).toEqual({ kind: 'list', type: null });
  });

  it('parses /search without a trailing slash', () => {
    expect(viewFromPath('/search')).toEqual({ kind: 'list', type: null });
  });

  it('parses /resource/:type/ as a list view narrowed to that type', () => {
    expect(viewFromPath('/resource/models/')).toEqual({
      kind: 'list',
      type: 'models',
    });
  });

  it('parses /resource/:type without a trailing slash', () => {
    expect(viewFromPath('/resource/tests')).toEqual({
      kind: 'list',
      type: 'tests',
    });
  });

  it('parses /details/:uniqueId/ as a detail view', () => {
    expect(viewFromPath('/details/model.acme.dim_users/')).toEqual({
      kind: 'detail',
      uniqueId: 'model.acme.dim_users',
    });
  });

  it('decodes URI-encoded resourceType segments', () => {
    expect(viewFromPath('/resource/semantic%20models/')).toEqual({
      kind: 'list',
      type: 'semantic models',
    });
  });

  it('decodes URI-encoded uniqueId segments', () => {
    expect(viewFromPath('/details/model.acme.dim%20users/')).toEqual({
      kind: 'detail',
      uniqueId: 'model.acme.dim users',
    });
  });

  it('treats /resource/source/:sourceName/ as a list view narrowed to source', () => {
    // SourceCollectionPage lives at this URL; the LocatePane "Sources" row
    // should stay highlighted while the user drills into a collection.
    expect(viewFromPath('/resource/source/raw_jaffle/')).toEqual({
      kind: 'list',
      type: 'source',
    });
  });

  it('treats nested /details paths as home (only single segment matches)', () => {
    expect(viewFromPath('/details/model.acme.dim_users/columns/')).toEqual({
      kind: 'home',
    });
  });
});
