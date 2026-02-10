/**
 * Conversation State
 *
 * Derives the current conversation from instanceStates based on currentInstanceId.
 * Each instance maintains its own conversation history.
 */

import { derived } from 'svelte/store';
import type { ConversationTurn, NotebookCell, ToolCell } from '$lib/types';
import {
	currentInstanceId,
	currentInstanceState,
	setInstanceConversation,
	appendInstanceTurns,
	setInstanceWaiting
} from './instances';

// =============================================================================
// Derived Stores (from per-instance state)
// =============================================================================

/** Current instance's conversation turns */
export const conversationTurns = derived(currentInstanceState, ($state): ConversationTurn[] => {
	return $state?.conversation ?? [];
});

/** Whether current instance is waiting for first message */
export const isWaiting = derived(currentInstanceState, ($state): boolean => {
	return $state?.isWaiting ?? true;
});

/** Map role to cell type */
function roleToCellType(role: string): NotebookCell['type'] {
	switch (role) {
		case 'User':
			return 'user';
		case 'Assistant':
			return 'assistant';
		case 'System':
			return 'system';
		case 'AgentProgress':
			return 'agent';
		case 'Progress':
			return 'progress';
		default:
			return 'unknown';
	}
}

/** Transform turns into Observable-style notebook cells, aggregating consecutive progress entries */
export const notebookCells = derived(conversationTurns, ($turns): NotebookCell[] => {
	const t0 = performance.now();
	const cells: NotebookCell[] = [];

	// Filter out skip entries, then process
	const filteredTurns = $turns.filter((turn) => !turn.skip && turn.role !== 'Skip');

	for (let i = 0; i < filteredTurns.length; i++) {
		const turn = filteredTurns[i];

		// For progress entries, aggregate consecutive ones
		if (turn.role === 'Progress' || turn.role === 'AgentProgress') {
			// Check if previous cell is also progress - if so, merge
			const prevCell = cells[cells.length - 1];
			if (prevCell && prevCell.type === 'progress') {
				// Aggregate: increment count and update content
				const currentCount = (prevCell.extra?.progressCount as number) ?? 1;
				const items = (prevCell.extra?.progressItems as string[]) ?? [prevCell.content];

				// Add this item if it's different from the last
				const newItem = turn.content;
				if (items[items.length - 1] !== newItem) {
					items.push(newItem);
				}

				prevCell.extra = {
					...prevCell.extra,
					progressCount: currentCount + 1,
					progressItems: items // Keep all items for explorer view
				};
				prevCell.content = `${currentCount + 1} events`;
				prevCell.timestamp = turn.timestamp; // Update to latest timestamp
				continue;
			}

			// First progress in a sequence
			const cell: NotebookCell = {
				id: turn.uuid ?? `turn-${i}`,
				type: 'progress',
				content: turn.content,
				timestamp: turn.timestamp,
				collapsed: false,
				extra: {
					progressCount: 1,
					progressItems: [turn.content],
					progressType: turn.progress_type ?? (turn.agent_id ? 'agent' : 'hook')
				}
			};

			if (turn.agent_id) cell.agentId = turn.agent_id;
			if (turn.agent_prompt) cell.agentPrompt = turn.agent_prompt;
			if (turn.hook_event) cell.hookEvent = turn.hook_event;

			cells.push(cell);
			continue;
		}

		// Regular cell processing
		const cell: NotebookCell = {
			id: turn.uuid ?? `turn-${i}`,
			type: roleToCellType(turn.role),
			content: turn.content,
			timestamp: turn.timestamp,
			collapsed: false
		};

		// If assistant message has tools, create tool cells
		if (turn.role === 'Assistant' && turn.tools.length > 0) {
			cell.toolCells = turn.tools.map(
				(toolName, toolIndex): ToolCell => ({
					id: `${cell.id}-tool-${toolIndex}`,
					name: toolName,
					input: {},
					status: 'complete',
					timestamp: turn.timestamp,
					canRerun: isRerunnable(toolName)
				})
			);
		}

		// Pass through extended thinking content
		if (turn.thinking) {
			cell.thinking = turn.thinking;
		}

		// Pass through multi-user attribution
		if (turn.attributed_to) {
			cell.attributed_to = turn.attributed_to;
		}

		// Pass through structural task reference
		if (turn.task_id != null) {
			cell.task_id = turn.task_id;
		}

		// Pass through entry type for unknown entries
		if (turn.entry_type) {
			cell.entryType = turn.entry_type;
		}

		// Pass through extra data for unknown entries
		if (turn.extra) {
			cell.extra = turn.extra;
		}

		// Pass through agent info for agent progress cells
		if (turn.agent_id) {
			cell.agentId = turn.agent_id;
		}
		if (turn.agent_prompt) {
			cell.agentPrompt = turn.agent_prompt;
		}
		if (turn.agent_msg_role) {
			cell.agentMsgRole = turn.agent_msg_role;
		}

		cells.push(cell);
	}

	const ms = performance.now() - t0;
	if (ms > 10) {
		console.warn(`[Conversation] notebookCells derivation (${cells.length} cells) took ${ms.toFixed(1)}ms`);
	}
	return cells;
});

/** Just the tool cells for quick access */
export const allToolCells = derived(notebookCells, ($cells): ToolCell[] => {
	return $cells.flatMap((cell) => cell.toolCells ?? []);
});

/** Group by tool type for stats */
export const toolStats = derived(allToolCells, ($tools) => {
	const counts = new Map<string, number>();
	$tools.forEach((tool) => {
		counts.set(tool.name, (counts.get(tool.name) ?? 0) + 1);
	});
	return counts;
});

// =============================================================================
// Helpers
// =============================================================================

function isRerunnable(toolName: string): boolean {
	const rerunnableTools = ['Read', 'Glob', 'Grep', 'Bash', 'WebFetch', 'WebSearch'];
	return rerunnableTools.includes(toolName);
}

// =============================================================================
// Actions (require instanceId - websocket.ts passes this)
// =============================================================================

/** Set conversation for a specific instance */
export function setConversation(instanceId: string, turns: ConversationTurn[]): void {
	const t0 = performance.now();
	setInstanceConversation(instanceId, turns);
	const ms = performance.now() - t0;
	if (ms > 20) {
		console.warn(`[Conversation] setConversation(${turns.length} turns) took ${ms.toFixed(1)}ms`);
	}
}

/** Append turns to a specific instance's conversation */
export function appendTurns(instanceId: string, newTurns: ConversationTurn[]): void {
	if (newTurns.length === 0) return;
	const t0 = performance.now();
	appendInstanceTurns(instanceId, newTurns);
	const ms = performance.now() - t0;
	if (ms > 20) {
		console.warn(`[Conversation] appendTurns(${newTurns.length} turns) took ${ms.toFixed(1)}ms`);
	}
}

/** Set waiting state for a specific instance */
export function setWaiting(instanceId: string, waiting: boolean): void {
	setInstanceWaiting(instanceId, waiting);
}

// Note: clearConversation is removed - no longer needed.
// Switching instances automatically shows that instance's preserved conversation.
// To clear an instance's conversation, call setConversation(instanceId, []).
