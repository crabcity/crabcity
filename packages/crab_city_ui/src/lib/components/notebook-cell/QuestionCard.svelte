<script lang="ts">
	import type { ToolCell } from '$lib/types';

	interface Props {
		tool: ToolCell;
	}

	let { tool }: Props = $props();

	let showRaw = $state(false);

	// Parse AskUserQuestion structured input
	interface QuestionOption {
		label: string;
		description?: string;
	}

	interface StructuredQuestion {
		question: string;
		header?: string;
		options: QuestionOption[];
		multiSelect?: boolean;
	}

	const questions = $derived((): StructuredQuestion[] => {
		const input = tool.input;
		// AskUserQuestion sends { questions: [...] }
		if (Array.isArray(input.questions)) {
			return input.questions as StructuredQuestion[];
		}
		// Fallback: single question shape
		if (typeof input.question === 'string') {
			return [{
				question: input.question as string,
				header: input.header as string | undefined,
				options: Array.isArray(input.options) ? input.options as QuestionOption[] : [],
				multiSelect: input.multiSelect as boolean | undefined,
			}];
		}
		return [];
	});

	const isResolved = $derived(!!tool.output);
	const isPending = $derived(!tool.output);

	// Extract answer values from tool output.
	// Format: User has answered your questions: "question"="answer", ...
	const answerValues: string[] = $derived.by(() => {
		if (!tool.output) return [];
		const matches = [...tool.output.matchAll(/="([^"]*)"/g)];
		return matches.map((m) => m[1]);
	});

	// Set of option labels that appear in the answer values
	const selectedLabels: Set<string> = $derived.by(() => {
		if (!tool.output) return new Set();
		const allLabels = questions().flatMap((q) => q.options.map((o) => o.label));
		return new Set(allLabels.filter((label) => answerValues.includes(label)));
	});

	// True when resolved but answer doesn't match any known option
	const isOtherSelected = $derived(isResolved && selectedLabels.size === 0);

	// For "Other" answers, show just the answer text (not the full protocol string)
	const resultText: string | null = $derived.by(() => {
		if (!tool.output) return null;
		if (answerValues.length > 0) return answerValues.join(', ');
		return tool.output;
	});
</script>

<div class="question-card" class:pending={isPending} class:resolved={isResolved}>
	{#if showRaw}
		<!-- Raw view: show tool data for debugging -->
		<div class="raw-view">
			<div class="raw-header">
				<span class="raw-title">RAW — ASKUSERQUESTION</span>
				<button class="toggle-raw" onclick={() => showRaw = false} title="Show rendered">◆</button>
			</div>
			<div class="raw-field">
				<span class="raw-label">INPUT</span>
				<pre class="raw-value">{JSON.stringify(tool.input, null, 2)}</pre>
			</div>
			<div class="raw-field">
				<span class="raw-label">OUTPUT</span>
				<pre class="raw-value">{tool.output ?? '(none)'}</pre>
			</div>
			<div class="raw-field">
				<span class="raw-label">PARSED ANSWERS</span>
				<pre class="raw-value">{answerValues.length > 0 ? JSON.stringify(answerValues) : '(none)'}</pre>
			</div>
			<div class="raw-field">
				<span class="raw-label">MATCHED LABELS</span>
				<pre class="raw-value">{selectedLabels.size > 0 ? JSON.stringify([...selectedLabels]) : '(none)'}</pre>
			</div>
			<div class="raw-field">
				<span class="raw-label">STATUS</span>
				<pre class="raw-value">{isResolved ? (isOtherSelected ? 'resolved (other)' : 'resolved') : 'pending'}</pre>
			</div>
		</div>
	{:else}
		<!-- Rendered view: structured Q/A card -->
		<div class="card-header">
			<button class="toggle-raw" onclick={() => showRaw = true} title="Show raw">◇</button>
		</div>
		{#each questions() as q, qi}
			<div class="question-block">
				{#if q.header}
					<div class="question-header">
						<span class="header-chip">{q.header}</span>
						{#if q.multiSelect}
							<span class="multi-badge">MULTI-SELECT</span>
						{/if}
					</div>
				{/if}

				<div class="question-text">{q.question}</div>

				{#if q.options.length > 0}
					<ol class="options-list">
						{#each q.options as opt, oi}
							<li
								class="option"
								class:first-option={!isResolved && oi === 0}
								class:selected-option={isResolved && selectedLabels.has(opt.label)}
								class:unselected-option={isResolved && !selectedLabels.has(opt.label)}
							>
								<span class="option-number">{oi + 1}.</span>
								<div class="option-body">
									<span class="option-label">{opt.label}</span>
									{#if opt.description}
										<span class="option-desc">{opt.description}</span>
									{/if}
								</div>
							</li>
						{/each}
						<!-- "Other" option is always implicitly available -->
						<li
							class="option other-option"
							class:selected-option={isOtherSelected}
							class:unselected-option={isResolved && !isOtherSelected}
						>
							<span class="option-number">{q.options.length + 1}.</span>
							<div class="option-body">
								<span class="option-label other-label">Other</span>
								<span class="option-desc">Custom text input</span>
							</div>
						</li>
					</ol>
				{/if}

				{#if qi < questions().length - 1}
					<div class="question-divider"></div>
				{/if}
			</div>
		{/each}

		{#if isResolved && isOtherSelected && resultText}
			<div class="result-section">
				<span class="result-label">ANSWERED</span>
				<pre class="result-value">{resultText}</pre>
			</div>
		{:else if isPending}
			<div class="pending-banner">
				<span class="pending-icon">⌨</span>
				<span class="pending-text">Switch to the Terminal view to answer this question</span>
			</div>
		{/if}
	{/if}
</div>

<style>
	.question-card {
		border: 1px solid var(--amber-600);
		border-radius: 4px;
		background: var(--surface-800);
		overflow: hidden;
		animation: card-on 0.3s ease-out;
	}

	.question-card.pending {
		border-color: var(--amber-500);
		box-shadow: 0 0 12px rgba(251, 146, 60, 0.08);
	}

	.question-card.resolved {
		border-color: var(--surface-border);
		opacity: 0.85;
	}

	/* ── Card header with raw toggle ──────────── */

	.card-header {
		display: flex;
		justify-content: flex-end;
		padding: 4px 6px 0 6px;
	}

	.toggle-raw {
		background: none;
		border: none;
		color: var(--text-muted);
		cursor: pointer;
		font-size: 12px;
		padding: 2px 6px;
		border-radius: 3px;
		opacity: 0.3;
		transition: all 0.15s ease;
	}

	.question-card:hover .toggle-raw {
		opacity: 0.8;
	}

	.toggle-raw:hover {
		background: var(--surface-500);
		color: var(--amber-400);
	}

	/* ── Raw view ─────────────────────────────── */

	.raw-view {
		padding: 0;
	}

	.raw-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 6px 10px;
		border-bottom: 1px solid var(--surface-border);
	}

	.raw-title {
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 0.15em;
		color: var(--amber-400);
	}

	.raw-field {
		padding: 6px 10px;
		border-bottom: 1px solid var(--surface-border);
	}

	.raw-field:last-child {
		border-bottom: none;
	}

	.raw-label {
		display: block;
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 0.15em;
		color: var(--text-muted);
		margin-bottom: 2px;
	}

	.raw-value {
		margin: 0;
		white-space: pre-wrap;
		word-break: break-all;
		font-family: inherit;
		font-size: 10px;
		line-height: 1.5;
		color: var(--text-secondary);
		max-height: 200px;
		overflow-y: auto;
	}

	/* ── Question blocks ──────────────────────── */

	.question-block {
		padding: 12px 14px;
	}

	.question-header {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-bottom: 8px;
	}

	.header-chip {
		display: inline-block;
		padding: 2px 8px;
		background: var(--tint-active);
		border: 1px solid var(--amber-600);
		border-radius: 3px;
		font-size: 9px;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.15em;
		color: var(--amber-400);
	}

	.multi-badge {
		font-size: 8px;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.15em;
		color: var(--text-muted);
		padding: 1px 6px;
		border: 1px solid var(--surface-border);
		border-radius: 2px;
	}

	.question-text {
		font-size: 12px;
		line-height: 1.5;
		color: var(--text-primary);
		margin-bottom: 10px;
	}

	.options-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.option {
		display: flex;
		align-items: flex-start;
		gap: 8px;
		padding: 6px 10px;
		border-radius: 3px;
		background: var(--surface-700);
		border: 1px solid transparent;
		transition: all 0.15s ease;
	}

	.option.first-option {
		border-color: var(--amber-600);
		background: var(--tint-active);
	}

	.option-number {
		flex-shrink: 0;
		min-width: 16px;
		font-size: 10px;
		font-weight: 600;
		color: var(--text-muted);
		text-align: right;
		line-height: 1.4;
	}

	.first-option .option-number {
		color: var(--amber-400);
	}

	.option-body {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.option-label {
		font-size: 11px;
		font-weight: 600;
		color: var(--text-primary);
		line-height: 1.4;
	}

	.first-option .option-label {
		color: var(--amber-400);
	}

	.option-desc {
		font-size: 10px;
		color: var(--text-muted);
		line-height: 1.4;
	}

	/* ── Resolved selection states ─────────────── */

	.option.selected-option {
		border-color: var(--amber-600);
		background: var(--tint-active);
	}

	.option.selected-option .option-number {
		color: var(--amber-400);
	}

	.option.selected-option .option-label {
		color: var(--amber-400);
	}

	.option.unselected-option {
		opacity: 0.4;
		border-color: transparent;
	}

	.other-option {
		opacity: 0.5;
	}

	.other-option.selected-option {
		opacity: 1;
	}

	.other-option.unselected-option {
		opacity: 0.4;
	}

	.other-label {
		font-style: italic;
	}

	.question-divider {
		height: 1px;
		background: var(--surface-border);
		margin: 10px 0;
	}

	/* ── Result section (answered) ──────────────── */

	.result-section {
		padding: 8px 14px;
		border-top: 1px solid var(--surface-border);
		background: var(--surface-700);
	}

	.result-label {
		display: block;
		font-size: 9px;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.15em;
		color: var(--status-green-text, var(--status-green));
		margin-bottom: 3px;
	}

	.result-value {
		margin: 0;
		white-space: pre-wrap;
		word-break: break-word;
		font-family: inherit;
		font-size: 11px;
		line-height: 1.5;
		color: var(--text-primary);
	}

	/* ── Pending banner ─────────────────────────── */

	.pending-banner {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 14px;
		border-top: 1px solid var(--amber-600);
		background: var(--tint-active);
	}

	.pending-icon {
		font-size: 12px;
		flex-shrink: 0;
	}

	.pending-text {
		font-size: 10px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--amber-400);
	}

	@keyframes card-on {
		0% { opacity: 0; filter: brightness(3); }
		30% { opacity: 0.5; filter: brightness(2); }
		60% { opacity: 0.8; filter: brightness(1.2); }
		100% { opacity: 1; filter: brightness(1); }
	}

	/* Mobile */
	@media (max-width: 639px) {
		.question-block {
			padding: 10px 12px;
		}

		.question-text {
			font-size: 11px;
		}

		.option {
			padding: 5px 8px;
		}

		.option-label {
			font-size: 10px;
		}

		.pending-text {
			font-size: 9px;
		}

		.toggle-raw {
			opacity: 0.6;
			padding: 4px 8px;
			font-size: 14px;
		}
	}

	/* Analog theme */
	:global([data-theme="analog"]) .question-card {
		background-color: var(--surface-800);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
		border-color: var(--surface-border);
	}

	:global([data-theme="analog"]) .question-card {
		animation: ink-bleed 0.5s cubic-bezier(0.1, 0.9, 0.2, 1);
	}

	@keyframes ink-bleed {
		0% { opacity: 0; transform: scaleY(0.95); }
		100% { opacity: 1; transform: scaleY(1); }
	}
</style>
