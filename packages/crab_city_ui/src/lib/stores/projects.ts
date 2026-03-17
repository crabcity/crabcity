/**
 * Project Grouping Store
 *
 * Groups instances by working_dir into logical "projects".
 * Purely derived — no persistence, no server changes.
 */

import { derived } from 'svelte/store';
import { instanceList } from './instances';
import { activeProjectId } from './layout';
import { projectHash } from '$lib/utils/project-id';
import type { Instance } from '$lib/types';

export interface Project {
  id: string;
  name: string;
  workingDir: string;
  instances: Instance[];
}

/** Extract basename from a path */
function basename(path: string): string {
  const parts = path.replace(/\/+$/, '').split('/');
  return parts[parts.length - 1] || path;
}

/** All projects, derived from instanceList grouped by working_dir */
export const projects = derived(instanceList, ($list) => {
  const groups = new Map<string, Instance[]>();
  for (const inst of $list) {
    const dir = inst.working_dir;
    if (!groups.has(dir)) groups.set(dir, []);
    groups.get(dir)!.push(inst);
  }
  return Array.from(groups.entries()).map(([dir, insts]) => ({
    id: projectHash(dir),
    name: basename(dir),
    workingDir: dir,
    instances: insts
  }));
});

/** The project containing the currently active layout */
export const currentProject = derived([projects, activeProjectId], ([$projects, $activeId]) => {
  if (!$activeId) return null;
  return $projects.find((p) => p.id === $activeId) ?? null;
});
