/**
 * Project Grouping Store
 *
 * Groups instances by working_dir into logical "projects".
 * Purely derived — no persistence, no server changes.
 */

import { derived } from 'svelte/store';
import { instanceList, currentInstanceId, instances } from './instances';
import type { Instance } from '$lib/types';

export interface Project {
	id: string;
	name: string;
	workingDir: string;
	instances: Instance[];
}

/** Simple string hash for stable project IDs */
function simpleHash(str: string): string {
	let hash = 0;
	for (let i = 0; i < str.length; i++) {
		const char = str.charCodeAt(i);
		hash = ((hash << 5) - hash) + char;
		hash |= 0;
	}
	return 'proj-' + Math.abs(hash).toString(36);
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
		id: simpleHash(dir),
		name: basename(dir),
		workingDir: dir,
		instances: insts
	}));
});

/** The project containing the currently selected instance */
export const currentProject = derived(
	[projects, currentInstanceId, instances],
	([$projects, $currentId, $instances]) => {
		if (!$currentId) return null;
		const inst = $instances.get($currentId);
		if (!inst) return null;
		return $projects.find(p => p.workingDir === inst.working_dir) ?? null;
	}
);
