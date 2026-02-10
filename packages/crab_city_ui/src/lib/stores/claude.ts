/**
 * Claude State Store
 *
 * The authoritative state of what Claude is doing.
 * Derived from the per-instance state in the instances map.
 */

import { derived, writable } from 'svelte/store';
import type { ClaudeState } from '$lib/types';
import { currentInstanceId, instances } from './instances';

// =============================================================================
// Core State - Derived from per-instance state with memoization
// =============================================================================

/**
 * Memoized Claude state for the focused instance.
 *
 * Uses a two-stage approach to prevent unnecessary re-renders:
 * 1. Raw derived store computes state from instances map
 * 2. Memoization layer only emits when state actually changes
 *
 * Without memoization, every StateChange for ANY instance would trigger
 * subscribers (since instances map reference changes), causing scroll
 * rubberbanding and unnecessary re-renders.
 */
const _claudeStateRaw = derived(
	[currentInstanceId, instances],
	([$instanceId, $instances]): ClaudeState => {
		if (!$instanceId) return { type: 'Idle' };
		const instance = $instances.get($instanceId);
		return instance?.claude_state ?? { type: 'Idle' };
	}
);

// Memoized store that only updates when state meaningfully changes
const _claudeStateMemo = writable<ClaudeState>({ type: 'Idle' });
let _lastStateKey: string | null = null;

_claudeStateRaw.subscribe(($state) => {
	// Create a stable key for comparison (type + tool name if applicable)
	const key = $state.type === 'ToolExecuting'
		? `${$state.type}:${$state.tool}`
		: $state.type;

	if (key !== _lastStateKey) {
		_lastStateKey = key;
		_claudeStateMemo.set($state);
	}
});

/** Current Claude state for the focused instance (memoized) */
export const claudeState = { subscribe: _claudeStateMemo.subscribe };

// =============================================================================
// Derived State
// =============================================================================

/** Is Claude currently active (thinking, responding, or executing)? */
export const isActive = derived(claudeState, ($state) =>
	$state.type === 'Thinking' || $state.type === 'Responding' || $state.type === 'ToolExecuting'
);

/** Is Claude in the thinking phase? */
export const isThinking = derived(claudeState, ($state) => $state.type === 'Thinking');

/** Is Claude executing a tool? */
export const isToolExecuting = derived(claudeState, ($state) => $state.type === 'ToolExecuting');

/** Current tool name if executing, null otherwise */
export const currentTool = derived(claudeState, ($state) =>
	$state.type === 'ToolExecuting' ? $state.tool : null
);

// =============================================================================
// State Updates
// =============================================================================

// Note: setClaudeState and resetClaudeState are removed.
// State is now derived from the instances map, which is updated
// via StateChange broadcasts from the server. No manual syncing needed.
