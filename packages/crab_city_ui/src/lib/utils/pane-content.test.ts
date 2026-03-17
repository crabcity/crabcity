import {
  getPaneInstanceId,
  getPaneWorkingDir,
  defaultContentForKind,
  migratePaneContentV3toV4
} from './pane-content.js';
import type { InstanceRef } from './pane-content.js';

// =============================================================================
// getPaneInstanceId
// =============================================================================

describe('getPaneInstanceId', () => {
  it('returns instanceId for terminal', () => {
    expect(getPaneInstanceId({ kind: 'terminal', instanceId: 'inst-1' })).toBe('inst-1');
  });

  it('returns null instanceId for terminal', () => {
    expect(getPaneInstanceId({ kind: 'terminal', instanceId: null })).toBeNull();
  });

  it('returns instanceId for conversation', () => {
    expect(getPaneInstanceId({ kind: 'conversation', instanceId: 'inst-2', viewMode: 'structured' })).toBe('inst-2');
  });

  it('returns null for file-explorer', () => {
    expect(getPaneInstanceId({ kind: 'file-explorer', workingDir: '/foo' })).toBeNull();
  });

  it('returns null for tasks', () => {
    expect(getPaneInstanceId({ kind: 'tasks', workingDir: '/bar' })).toBeNull();
  });

  it('returns null for git', () => {
    expect(getPaneInstanceId({ kind: 'git', workingDir: '/baz' })).toBeNull();
  });

  it('returns null for landing', () => {
    expect(getPaneInstanceId({ kind: 'landing' })).toBeNull();
  });

  it('returns null for settings', () => {
    expect(getPaneInstanceId({ kind: 'settings' })).toBeNull();
  });

  it('returns null for chat', () => {
    expect(getPaneInstanceId({ kind: 'chat', scope: 'global' })).toBeNull();
  });

  it('returns null for file-viewer', () => {
    expect(getPaneInstanceId({ kind: 'file-viewer', filePath: '/x.ts' })).toBeNull();
  });

  it('returns null for picker', () => {
    expect(getPaneInstanceId({ kind: 'picker' })).toBeNull();
  });
});

// =============================================================================
// getPaneWorkingDir
// =============================================================================

describe('getPaneWorkingDir', () => {
  const instances: ReadonlyMap<string, InstanceRef> = new Map([
    ['inst-1', { working_dir: '/projects/alpha' }],
    ['inst-2', { working_dir: '/projects/beta' }]
  ]);

  const emptyInstances: ReadonlyMap<string, InstanceRef> = new Map();

  it('returns workingDir directly for file-explorer', () => {
    expect(getPaneWorkingDir({ kind: 'file-explorer', workingDir: '/foo' }, emptyInstances)).toBe('/foo');
  });

  it('returns workingDir directly for tasks', () => {
    expect(getPaneWorkingDir({ kind: 'tasks', workingDir: '/bar' }, emptyInstances)).toBe('/bar');
  });

  it('returns workingDir directly for git', () => {
    expect(getPaneWorkingDir({ kind: 'git', workingDir: '/baz' }, emptyInstances)).toBe('/baz');
  });

  it('returns null workingDir for directory-bound pane with null', () => {
    expect(getPaneWorkingDir({ kind: 'file-explorer', workingDir: null }, emptyInstances)).toBeNull();
  });

  it('resolves working_dir from instance for terminal', () => {
    expect(getPaneWorkingDir({ kind: 'terminal', instanceId: 'inst-1' }, instances)).toBe('/projects/alpha');
  });

  it('resolves working_dir from instance for conversation', () => {
    expect(getPaneWorkingDir({ kind: 'conversation', instanceId: 'inst-2', viewMode: 'raw' }, instances)).toBe(
      '/projects/beta'
    );
  });

  it('returns null for terminal with no matching instance', () => {
    expect(getPaneWorkingDir({ kind: 'terminal', instanceId: 'inst-99' }, instances)).toBeNull();
  });

  it('returns null for terminal with null instanceId', () => {
    expect(getPaneWorkingDir({ kind: 'terminal', instanceId: null }, instances)).toBeNull();
  });

  it('returns null for landing', () => {
    expect(getPaneWorkingDir({ kind: 'landing' }, instances)).toBeNull();
  });

  it('returns null for settings', () => {
    expect(getPaneWorkingDir({ kind: 'settings' }, instances)).toBeNull();
  });

  it('returns null for chat', () => {
    expect(getPaneWorkingDir({ kind: 'chat', scope: 'global' }, instances)).toBeNull();
  });

  it('returns null for picker without source context', () => {
    expect(getPaneWorkingDir({ kind: 'picker' }, instances)).toBeNull();
  });

  it('returns sourceWorkingDir for picker with source context', () => {
    expect(getPaneWorkingDir({ kind: 'picker', sourceWorkingDir: '/projects/alpha' }, instances)).toBe(
      '/projects/alpha'
    );
  });

  it('returns null for picker with null sourceWorkingDir', () => {
    expect(getPaneWorkingDir({ kind: 'picker', sourceWorkingDir: null }, instances)).toBeNull();
  });

  it('handles instance without working_dir property', () => {
    const sparse = new Map([['inst-x', {} as InstanceRef]]);
    expect(getPaneWorkingDir({ kind: 'terminal', instanceId: 'inst-x' }, sparse)).toBeNull();
  });
});

// =============================================================================
// defaultContentForKind
// =============================================================================

describe('defaultContentForKind', () => {
  it('creates landing content', () => {
    expect(defaultContentForKind('landing', null)).toEqual({ kind: 'landing' });
  });

  it('creates terminal with instanceId', () => {
    expect(defaultContentForKind('terminal', 'inst-1')).toEqual({ kind: 'terminal', instanceId: 'inst-1' });
  });

  it('creates terminal with null instanceId', () => {
    expect(defaultContentForKind('terminal', null)).toEqual({ kind: 'terminal', instanceId: null });
  });

  it('creates conversation with instanceId and structured viewMode', () => {
    expect(defaultContentForKind('conversation', 'inst-1')).toEqual({
      kind: 'conversation',
      instanceId: 'inst-1',
      viewMode: 'structured'
    });
  });

  it('creates file-explorer with workingDir', () => {
    expect(defaultContentForKind('file-explorer', null, '/projects/alpha')).toEqual({
      kind: 'file-explorer',
      workingDir: '/projects/alpha'
    });
  });

  it('creates file-explorer with null workingDir when not provided', () => {
    expect(defaultContentForKind('file-explorer', null)).toEqual({ kind: 'file-explorer', workingDir: null });
  });

  it('creates tasks with workingDir', () => {
    expect(defaultContentForKind('tasks', null, '/proj')).toEqual({ kind: 'tasks', workingDir: '/proj' });
  });

  it('creates git with workingDir', () => {
    expect(defaultContentForKind('git', null, '/proj')).toEqual({ kind: 'git', workingDir: '/proj' });
  });

  it('directory-bound kinds ignore instanceId parameter', () => {
    const result = defaultContentForKind('file-explorer', 'inst-1', '/foo');
    expect(result).toEqual({ kind: 'file-explorer', workingDir: '/foo' });
    expect('instanceId' in result).toBe(false);
  });

  it('instance-bound kinds ignore workingDir parameter', () => {
    const result = defaultContentForKind('terminal', 'inst-1', '/foo');
    expect(result).toEqual({ kind: 'terminal', instanceId: 'inst-1' });
    expect('workingDir' in result).toBe(false);
  });

  it('creates chat with instanceId as scope', () => {
    expect(defaultContentForKind('chat', 'inst-1')).toEqual({ kind: 'chat', scope: 'inst-1' });
  });

  it('creates chat with global scope when instanceId is null', () => {
    expect(defaultContentForKind('chat', null)).toEqual({ kind: 'chat', scope: 'global' });
  });

  it('creates file-viewer with null filePath', () => {
    expect(defaultContentForKind('file-viewer', null)).toEqual({ kind: 'file-viewer', filePath: null });
  });

  it('creates settings', () => {
    expect(defaultContentForKind('settings', null)).toEqual({ kind: 'settings' });
  });

  it('creates picker', () => {
    expect(defaultContentForKind('picker', null)).toEqual({ kind: 'picker' });
  });
});

// =============================================================================
// migratePaneContentV3toV4
// =============================================================================

describe('migratePaneContentV3toV4', () => {
  it('migrates file-explorer from instanceId to workingDir', () => {
    const legacy = { kind: 'file-explorer', instanceId: 'inst-1' };
    expect(migratePaneContentV3toV4(legacy)).toEqual({ kind: 'file-explorer', workingDir: null });
  });

  it('migrates tasks from instanceId to workingDir', () => {
    const legacy = { kind: 'tasks', instanceId: 'inst-2' };
    expect(migratePaneContentV3toV4(legacy)).toEqual({ kind: 'tasks', workingDir: null });
  });

  it('migrates git from instanceId to workingDir', () => {
    const legacy = { kind: 'git', instanceId: null };
    expect(migratePaneContentV3toV4(legacy)).toEqual({ kind: 'git', workingDir: null });
  });

  it('returns null for already-migrated file-explorer', () => {
    const current = { kind: 'file-explorer', workingDir: '/foo' };
    expect(migratePaneContentV3toV4(current)).toBeNull();
  });

  it('returns null for terminal (not a directory-bound kind)', () => {
    const terminal = { kind: 'terminal', instanceId: 'inst-1' };
    expect(migratePaneContentV3toV4(terminal)).toBeNull();
  });

  it('returns null for conversation', () => {
    const convo = { kind: 'conversation', instanceId: 'inst-1', viewMode: 'structured' };
    expect(migratePaneContentV3toV4(convo)).toBeNull();
  });

  it('returns null for landing', () => {
    expect(migratePaneContentV3toV4({ kind: 'landing' })).toBeNull();
  });

  it('returns null for settings', () => {
    expect(migratePaneContentV3toV4({ kind: 'settings' })).toBeNull();
  });
});
