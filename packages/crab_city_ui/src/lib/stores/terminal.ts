/**
 * Terminal Output Store
 *
 * Instance-scoped terminal I/O that replaces global window events.
 * Each instance has its own output buffer - no crosstalk between terminals.
 */

import { writable, derived, get } from 'svelte/store';
import { currentInstanceId } from './instances';

// =============================================================================
// Types
// =============================================================================

interface TerminalBuffer {
	/** Pending output chunks to be written to terminal */
	chunks: string[];
	/** Whether terminal should be cleared before next write */
	shouldClear: boolean;
}

// =============================================================================
// Internal State
// =============================================================================

/** Map of instanceId -> output buffer */
const terminalBuffers = writable<Map<string, TerminalBuffer>>(new Map());

/**
 * Maximum chunks to buffer per instance.
 *
 * MAX_BUFFER_SIZE of 10000 because:
 * 1. Average chunk is 50-100 bytes
 * 2. 10000 * 100 = 1MB max per terminal (acceptable)
 * 3. Prevents unbounded growth if output never consumed
 * 4. FIFO eviction loses old output (not ideal but better than OOM)
 */
const MAX_BUFFER_SIZE = 10000;

// =============================================================================
// Write Functions (called by WebSocket)
// =============================================================================

/**
 * Write output to a specific instance's terminal buffer.
 * Called by WebSocket handler when receiving terminal output.
 */
export function writeTerminalOutput(instanceId: string, data: string): void {
	terminalBuffers.update((buffers) => {
		let buffer = buffers.get(instanceId);
		if (!buffer) {
			buffer = { chunks: [], shouldClear: false };
		}

		buffer.chunks.push(data);

		// Prevent unbounded growth
		if (buffer.chunks.length > MAX_BUFFER_SIZE) {
			buffer.chunks = buffer.chunks.slice(-MAX_BUFFER_SIZE);
		}

		buffers.set(instanceId, buffer);
		return new Map(buffers);
	});
}

/**
 * Write history output (on focus switch) - clears terminal first.
 */
export function writeTerminalHistory(instanceId: string, data: string): void {
	terminalBuffers.update((buffers) => {
		buffers.set(instanceId, {
			chunks: [data],
			shouldClear: true
		});
		return new Map(buffers);
	});
}

// =============================================================================
// Read Functions (called by Terminal component)
// =============================================================================

/**
 * Get pending output for the current instance.
 * Returns { chunks, shouldClear } and marks as consumed.
 */
export function consumeTerminalOutput(instanceId: string): TerminalBuffer {
	const buffers = get(terminalBuffers);
	const buffer = buffers.get(instanceId);

	if (!buffer || (buffer.chunks.length === 0 && !buffer.shouldClear)) {
		return { chunks: [], shouldClear: false };
	}

	// Clear the buffer after consumption
	terminalBuffers.update((b) => {
		b.set(instanceId, { chunks: [], shouldClear: false });
		return new Map(b);
	});

	return buffer;
}

/**
 * Check if there's pending output for an instance.
 */
export function hasPendingOutput(instanceId: string): boolean {
	const buffers = get(terminalBuffers);
	const buffer = buffers.get(instanceId);
	return buffer ? buffer.chunks.length > 0 || buffer.shouldClear : false;
}

// =============================================================================
// Derived Stores
// =============================================================================

/**
 * Reactive store that signals when current instance has pending output.
 * Terminal component can subscribe to this to know when to consume.
 */
export const currentTerminalHasOutput = derived(
	[terminalBuffers, currentInstanceId],
	([$buffers, $instanceId]) => {
		if (!$instanceId) return false;
		const buffer = $buffers.get($instanceId);
		return buffer ? buffer.chunks.length > 0 || buffer.shouldClear : false;
	}
);

// =============================================================================
// Cleanup
// =============================================================================

/**
 * Clear buffer for an instance (when instance is deleted).
 */
export function deleteTerminalBuffer(instanceId: string): void {
	terminalBuffers.update((buffers) => {
		buffers.delete(instanceId);
		return new Map(buffers);
	});
}
