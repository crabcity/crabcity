/**
 * Pure helpers for PaneContent — no store dependencies.
 *
 * These functions operate on the PaneContent discriminated union and are
 * the canonical way to extract instance/directory context from a pane.
 * Extracted from stores/layout.ts so they can be unit-tested without
 * importing Svelte stores or SvelteKit modules.
 */

// =============================================================================
// Types (re-exported by stores/layout.ts)
// =============================================================================

export type PaneContentKind =
  | 'landing'
  | 'terminal'
  | 'conversation'
  | 'file-explorer'
  | 'chat'
  | 'tasks'
  | 'file-viewer'
  | 'git'
  | 'settings'
  | 'picker';

export type PaneContent =
  | { kind: 'landing' }
  | { kind: 'terminal'; instanceId: string | null }
  | { kind: 'conversation'; instanceId: string | null; viewMode: 'structured' | 'raw' }
  | { kind: 'file-viewer'; filePath: string | null; lineNumber?: number }
  | { kind: 'file-explorer'; workingDir: string | null }
  | { kind: 'chat'; scope: 'global' | string }
  | { kind: 'tasks'; workingDir: string | null }
  | { kind: 'git'; workingDir: string | null }
  | { kind: 'settings' }
  | { kind: 'picker'; sourceWorkingDir?: string | null };

/** Minimal instance shape needed by getPaneWorkingDir */
export interface InstanceRef {
  working_dir?: string;
}

// =============================================================================
// Helpers
// =============================================================================

/** Extract instanceId from a PaneContent (terminal/conversation only) */
export function getPaneInstanceId(content: PaneContent): string | null {
  if (content.kind === 'terminal' || content.kind === 'conversation') return content.instanceId;
  return null;
}

/**
 * Extract workingDir from a PaneContent.
 *
 * Directory-bound kinds (file-explorer, tasks, git) return their workingDir directly.
 * Instance-bound kinds (terminal, conversation) resolve via the provided instance map.
 * Picker panes return their sourceWorkingDir.
 * All other kinds return null.
 */
export function getPaneWorkingDir(content: PaneContent, instanceMap: ReadonlyMap<string, InstanceRef>): string | null {
  if ('workingDir' in content) return content.workingDir;
  if (content.kind === 'picker') {
    return content.sourceWorkingDir ?? null;
  }
  const id = getPaneInstanceId(content);
  if (!id) return null;
  return instanceMap.get(id)?.working_dir ?? null;
}

/** Construct default PaneContent for a given kind */
export function defaultContentForKind(
  kind: PaneContentKind,
  instanceId: string | null,
  workingDir: string | null = null
): PaneContent {
  switch (kind) {
    case 'landing':
      return { kind: 'landing' };
    case 'terminal':
      return { kind: 'terminal', instanceId };
    case 'file-explorer':
    case 'tasks':
    case 'git':
      return { kind, workingDir };
    case 'conversation':
      return { kind: 'conversation', instanceId, viewMode: 'structured' };
    case 'file-viewer':
      return { kind: 'file-viewer', filePath: null };
    case 'chat':
      return { kind: 'chat', scope: instanceId ?? 'global' };
    case 'settings':
      return { kind: 'settings' };
    case 'picker':
      return { kind: 'picker' };
  }
}

/**
 * Migrate a legacy pane content record (v3 schema) to v4.
 * Converts directory-bound kinds from instanceId to workingDir.
 * Returns null if no migration was needed.
 */
export function migratePaneContentV3toV4(content: Record<string, unknown>): PaneContent | null {
  const kind = content['kind'];
  if (
    (kind === 'file-explorer' || kind === 'tasks' || kind === 'git') &&
    'instanceId' in content &&
    !('workingDir' in content)
  ) {
    return { kind: kind as 'file-explorer' | 'tasks' | 'git', workingDir: null };
  }
  return null;
}
