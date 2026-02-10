<script lang="ts">
	import {
		fileViewerState,
		isFileViewerOpen,
		currentFilePath,
		currentFileContent,
		currentFileLanguage,
		currentLineNumber,
		currentDiffData,
		currentViewMode,
		isDiffLoading,
		diffError,
		closeFileViewer,
		goToLine,
		toggleViewMode,
		setDiffData,
		setDiffError,
		setFileContent,
		rootDirectory,
		fetchFileContent
	} from '$lib/stores/files';
	import { diffEngine } from '$lib/stores/settings';
	import { gitFileStatuses, fetchGitDiff, gitDiff } from '$lib/stores/git';
	import { currentInstance } from '$lib/stores/instances';
	import { updateUrl } from '$lib/utils/url';
	import DiffView from './file-viewer/DiffView.svelte';
	import SourceView from './file-viewer/SourceView.svelte';

	// Panel width state with resize support
	let panelWidth = $state(Math.min(800, window.innerWidth * 0.5));
	let isResizing = $state(false);
	let startX = $state(0);
	let startWidth = $state(0);

	// Content container for scroll control
	let contentEl: HTMLElement | undefined = $state();
	let sourceViewRef: SourceView | undefined = $state();

	// Refresh UX state
	let refreshStatus: 'idle' | 'loading' | 'done' = $state('idle');
	let showRefreshOverlay = $state(false);
	let overlayTimer: ReturnType<typeof setTimeout> | null = null;
	let minimumTimer: ReturnType<typeof setTimeout> | null = null;
	let doneTimer: ReturnType<typeof setTimeout> | null = null;
	let overlayShownAt: number = 0;

	// Markdown detection
	const isMarkdown = $derived(
		$currentFileLanguage === 'markdown' ||
		$currentFileLanguage === 'md' ||
		$currentFilePath?.endsWith('.md') === true ||
		$currentFilePath?.endsWith('.markdown') === true
	);

	// Resize handlers
	function startResize(e: MouseEvent) {
		isResizing = true;
		startX = e.clientX;
		startWidth = panelWidth;
		document.body.style.cursor = 'col-resize';
		document.body.style.userSelect = 'none';
	}

	function handleMouseMove(e: MouseEvent) {
		if (!isResizing) return;
		const deltaX = startX - e.clientX;
		const newWidth = Math.max(320, Math.min(startWidth + deltaX, window.innerWidth * 0.8));
		panelWidth = newWidth;
	}

	function stopResize() {
		if (isResizing) {
			isResizing = false;
			document.body.style.cursor = '';
			document.body.style.userSelect = '';
		}
	}

	function getFilename(path: string | null): string {
		if (!path) return 'File';
		return path.split('/').pop() ?? 'File';
	}

	function getRelativePath(absPath: string | null): string {
		if (!absPath) return '';
		const root = $rootDirectory;
		if (absPath.startsWith(root)) {
			const rel = absPath.slice(root.length);
			return rel.startsWith('/') ? rel.slice(1) : rel;
		}
		return absPath;
	}

	// Scroll to line when lineNumber changes
	$effect(() => {
		const lineNum = $currentLineNumber;
		if (lineNum && contentEl) {
			requestAnimationFrame(() => {
				const lineEl = contentEl?.querySelector(`[data-line="${lineNum}"]`);
				if (lineEl) {
					lineEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
					lineEl.classList.add('highlight-line');
					setTimeout(() => lineEl.classList.remove('highlight-line'), 2000);
				}
			});
		}
	});

	// Copy path feedback
	let pathCopied = $state(false);
	let pathCopyTimer: ReturnType<typeof setTimeout> | null = null;

	async function copyPath() {
		const rel = getRelativePath($currentFilePath);
		if (!rel) return;
		try {
			await navigator.clipboard.writeText(rel);
		} catch {
			const textarea = document.createElement('textarea');
			textarea.value = rel;
			document.body.appendChild(textarea);
			textarea.select();
			document.execCommand('copy');
			document.body.removeChild(textarea);
		}
		pathCopied = true;
		if (pathCopyTimer) clearTimeout(pathCopyTimer);
		pathCopyTimer = setTimeout(() => { pathCopied = false; }, 1500);
	}

	async function copyContent() {
		if (!$currentFileContent) return;
		try {
			await navigator.clipboard.writeText($currentFileContent);
		} catch {
			const textarea = document.createElement('textarea');
			textarea.value = $currentFileContent;
			document.body.appendChild(textarea);
			textarea.select();
			document.execCommand('copy');
			document.body.removeChild(textarea);
		}
	}

	// Line count from source view or content
	const lineCount = $derived($currentFileContent?.split('\n').length ?? 0);

	// Whether the current file has git changes
	const fileHasGitChanges = $derived.by(() => {
		const relPath = getRelativePath($currentFilePath);
		return relPath ? $gitFileStatuses.has(relPath) : false;
	});

	// Handle diff toggle
	function handleDiffToggle() {
		if ($currentViewMode === 'diff') {
			toggleViewMode();
			if (!$currentFileContent && $currentFilePath) {
				fetchFileContent($currentFilePath)
					.then((content) => setFileContent(content))
					.catch(() => setFileContent('Failed to load file content'));
			}
			return;
		}

		if ($currentDiffData) {
			toggleViewMode();
			return;
		}

		const instance = $currentInstance;
		const filePath = $currentFilePath;
		if (!instance || !filePath) return;

		const relPath = getRelativePath(filePath);

		fileViewerState.update((s) => ({
			...s,
			viewMode: 'diff',
			diffLoading: true,
			diffError: null
		}));
		updateUrl({ view: 'diff' });

		fetchGitDiff(instance.id, undefined, relPath, $diffEngine)
			.then(() => {
				if ($currentFilePath !== filePath) return;
				const diff = $gitDiff;
				if (diff && diff.files.length > 0) {
					setDiffData(diff.files[0]);
				} else {
					setDiffError('No changes found');
				}
			})
			.catch(() => {
				if ($currentFilePath !== filePath) return;
				setDiffError();
			});
	}

	// Refresh timer helpers
	function clearRefreshTimers() {
		if (overlayTimer) { clearTimeout(overlayTimer); overlayTimer = null; }
		if (minimumTimer) { clearTimeout(minimumTimer); minimumTimer = null; }
		if (doneTimer) { clearTimeout(doneTimer); doneTimer = null; }
	}

	function settleRefresh() {
		const now = Date.now();
		const elapsed = now - overlayShownAt;
		if (showRefreshOverlay && elapsed < 500) {
			minimumTimer = setTimeout(() => {
				showRefreshOverlay = false;
				refreshStatus = 'done';
				doneTimer = setTimeout(() => { refreshStatus = 'idle'; }, 600);
			}, 500 - elapsed);
		} else {
			showRefreshOverlay = false;
			refreshStatus = 'done';
			doneTimer = setTimeout(() => { refreshStatus = 'idle'; }, 600);
		}
	}

	function handleEngineToggle() {
		const cycle: Record<string, 'standard' | 'patience' | 'structural'> = {
			structural: 'patience',
			patience: 'standard',
			standard: 'structural'
		};
		const newEngine = cycle[$diffEngine] ?? 'structural';
		diffEngine.set(newEngine);

		const instance = $currentInstance;
		const filePath = $currentFilePath;
		if (!instance || !filePath) return;

		const relPath = getRelativePath(filePath);

		clearRefreshTimers();
		refreshStatus = 'loading';
		showRefreshOverlay = false;

		overlayTimer = setTimeout(() => {
			if (refreshStatus === 'loading') {
				showRefreshOverlay = true;
				overlayShownAt = Date.now();
			}
		}, 200);

		fileViewerState.update((s) => ({
			...s,
			diffLoading: true,
			diffError: null
		}));

		fetchGitDiff(instance.id, undefined, relPath, newEngine)
			.then(() => {
				if ($currentFilePath !== filePath) return;
				const diff = $gitDiff;
				if (diff && diff.files.length > 0) {
					setDiffData(diff.files[0]);
				} else {
					setDiffError('No changes found');
				}
				settleRefresh();
			})
			.catch(() => {
				if ($currentFilePath !== filePath) return;
				setDiffError();
				settleRefresh();
			});
	}

	const isErrorContent = $derived(
		$currentFileContent?.startsWith('Security restriction:') ||
		$currentFileContent?.startsWith('Access denied:') ||
		$currentFileContent?.startsWith('Failed to load')
	);
</script>

<svelte:window onmousemove={handleMouseMove} onmouseup={stopResize} />

{#if $isFileViewerOpen}
	<button class="backdrop" onclick={closeFileViewer} aria-label="Close file viewer"></button>

	<aside class="file-viewer-panel" style="width: {panelWidth}px">
		<button class="resize-handle" onmousedown={startResize} aria-label="Resize panel"></button>

		<!-- Header -->
		<header class="panel-header">
			<button class="file-info" onclick={copyPath} title="Copy path to clipboard">
				<span class="file-icon">📄</span>
				<div class="file-meta">
					<span class="filename">{getFilename($currentFilePath)}</span>
					<span class="filepath" class:copied={pathCopied}>
						{#if pathCopied}
							Copied!
						{:else}
							{getRelativePath($currentFilePath)}
						{/if}
					</span>
				</div>
			</button>
			<div class="header-actions">
				{#if $currentFileLanguage && $currentViewMode !== 'diff'}
					<span class="language-badge">{$currentFileLanguage}</span>
				{/if}
				{#if $currentViewMode !== 'diff'}
					<span class="line-count">{lineCount} lines</span>
				{/if}
				{#if $currentDiffData || fileHasGitChanges || $currentViewMode === 'diff'}
					<button
						class="action-btn diff-toggle"
						class:active={$currentViewMode === 'diff'}
						onclick={handleDiffToggle}
						title={$currentViewMode === 'diff' ? 'Show file content' : 'Show diff'}
					>
						<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
							<path d="M12 3v18M3 12h18" />
						</svg>
						<span class="diff-label">Diff</span>
					</button>
				{/if}
				{#if isMarkdown}
					<button
						class="action-btn preview-toggle"
						class:active={sourceViewRef?.getShowPreview()}
						onclick={() => sourceViewRef?.togglePreview()}
						title={sourceViewRef?.getShowPreview() ? 'Show source' : 'Show preview'}
					>
						{#if sourceViewRef?.getShowPreview()}
							<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
								<polyline points="16 18 22 12 16 6"></polyline>
								<polyline points="8 6 2 12 8 18"></polyline>
							</svg>
						{:else}
							<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
								<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path>
								<circle cx="12" cy="12" r="3"></circle>
							</svg>
						{/if}
					</button>
				{/if}
				<button class="action-btn" onclick={copyContent} title="Copy file content">
					<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
						<rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
						<path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
					</svg>
				</button>
				<button class="close-btn" onclick={closeFileViewer} aria-label="Close file viewer">
					<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
						<line x1="18" y1="6" x2="6" y2="18"></line>
						<line x1="6" y1="6" x2="18" y2="18"></line>
					</svg>
				</button>
			</div>
		</header>

		<!-- Content -->
		<div class="panel-content" bind:this={contentEl}>
			{#if $currentViewMode === 'diff' && $isDiffLoading && !$currentDiffData}
				<div class="loading-state">
					<div class="spinner"></div>
					<p>Loading diff...</p>
				</div>
			{:else if $currentViewMode === 'diff' && $diffError && !$currentDiffData}
				<div class="diff-empty-state">
					<span class="diff-empty-text">{$diffError}</span>
				</div>
			{:else if $currentViewMode === 'diff' && $currentDiffData}
				<DiffView
					diffData={$currentDiffData}
					diffEngine={$diffEngine}
					actualEngine={$gitDiff?.engine}
					{refreshStatus}
					{showRefreshOverlay}
					onengineToggle={handleEngineToggle}
				/>
			{:else if $currentFileContent}
				<SourceView
					bind:this={sourceViewRef}
					content={$currentFileContent}
					language={$currentFileLanguage ?? ''}
					lineNumber={$currentLineNumber}
					isError={!!isErrorContent}
					{isMarkdown}
				/>
			{:else}
				<div class="loading-state">
					<div class="spinner"></div>
					<p>Loading file content...</p>
				</div>
			{/if}
		</div>

		<!-- Footer -->
		<footer class="panel-footer">
			<div class="line-jump">
				<label for="line-input">Go to line:</label>
				<input
					id="line-input"
					type="number"
					min="1"
					max={lineCount}
					placeholder="#"
					onkeydown={(e) => {
						if (e.key === 'Enter') {
							const target = e.currentTarget as HTMLInputElement;
							const line = parseInt(target.value, 10);
							if (line >= 1 && line <= lineCount) {
								goToLine(line);
								target.value = '';
							}
						}
					}}
				/>
			</div>
			{#if $currentLineNumber}
				<span class="current-line">Line {$currentLineNumber}</span>
			{/if}
		</footer>
	</aside>
{/if}

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		background: var(--backdrop);
		z-index: 100;
		border: none;
		cursor: default;
	}

	.file-viewer-panel {
		position: fixed;
		top: 0;
		right: 0;
		bottom: 0;
		display: flex;
		flex-direction: column;
		background: var(--surface-900);
		border-left: 1px solid var(--surface-border);
		z-index: 101;
		min-width: 320px;
		max-width: 85vw;
		box-shadow: var(--shadow-panel);
		animation: slideIn 0.2s ease-out;
	}

	@media (min-width: 1400px) {
		.file-viewer-panel { max-width: 90vw; }
	}

	@keyframes slideIn {
		from { transform: translateX(100%); opacity: 0; }
		to { transform: translateX(0); opacity: 1; }
	}

	.resize-handle {
		position: absolute;
		left: -4px;
		top: 0;
		bottom: 0;
		width: 8px;
		cursor: col-resize;
		background: transparent;
		border: none;
		padding: 0;
		transition: background 0.15s ease;
		z-index: 10;
	}

	.resize-handle:hover,
	.resize-handle:active {
		background: var(--tint-selection);
	}

	/* Header */
	.panel-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 12px 16px;
		background: linear-gradient(180deg, var(--surface-700) 0%, var(--surface-800) 100%);
		border-bottom: 1px solid var(--surface-border);
		flex-shrink: 0;
	}

	.file-info {
		display: flex;
		align-items: center;
		gap: 10px;
		min-width: 0;
		background: none;
		border: none;
		padding: 0;
		cursor: pointer;
		text-align: left;
		border-radius: 6px;
		transition: background 0.15s;
	}

	.file-info:hover { background: var(--tint-hover); }

	.file-icon { font-size: 18px; flex-shrink: 0; }

	.file-meta {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.filename {
		font-size: 13px;
		font-weight: 600;
		color: var(--amber-400);
		text-shadow: var(--emphasis);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.filepath {
		font-size: 10px;
		color: var(--text-muted);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		letter-spacing: 0.02em;
		transition: color 0.15s;
	}

	.filepath.copied { color: var(--amber-400); }

	.header-actions {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-shrink: 0;
	}

	.language-badge {
		padding: 3px 8px;
		background: var(--surface-600);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		font-size: 9px;
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--purple-400);
	}

	.line-count {
		font-size: 10px;
		color: var(--text-muted);
		letter-spacing: 0.05em;
	}

	.action-btn,
	.close-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		background: none;
		border: 1px solid transparent;
		border-radius: 4px;
		color: var(--text-secondary);
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.action-btn:hover,
	.close-btn:hover {
		background: var(--surface-600);
		border-color: var(--surface-border);
		color: var(--amber-400);
	}

	.action-btn.preview-toggle.active {
		background: var(--surface-600);
		border-color: var(--amber-600);
		color: var(--amber-400);
	}

	.action-btn svg,
	.close-btn svg { width: 16px; height: 16px; }

	.diff-toggle { gap: 4px; }

	.diff-toggle.active {
		background: var(--surface-600);
		border-color: var(--amber-600);
		color: var(--amber-400);
	}

	.diff-label {
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	/* Content area */
	.panel-content {
		flex: 1;
		overflow: auto;
		background: var(--surface-800);
	}

	.panel-content::-webkit-scrollbar { width: 8px; height: 8px; }
	.panel-content::-webkit-scrollbar-track { background: var(--surface-900); }
	.panel-content::-webkit-scrollbar-thumb { background: var(--surface-400); border-radius: 4px; }
	.panel-content::-webkit-scrollbar-thumb:hover { background: var(--amber-600); }

	/* Loading / empty states */
	.loading-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		height: 100%;
		gap: 12px;
		color: var(--text-muted);
	}

	.spinner {
		width: 24px;
		height: 24px;
		border: 2px solid var(--surface-border);
		border-top-color: var(--amber-400);
		border-radius: 50%;
		animation: spin 1s linear infinite;
	}

	@keyframes spin { to { transform: rotate(360deg); } }

	.loading-state p {
		font-size: 12px;
		letter-spacing: 0.05em;
	}

	.diff-empty-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		height: 100%;
		padding: 32px 16px;
		color: var(--text-muted);
	}

	.diff-empty-text {
		font-size: 12px;
		letter-spacing: 0.05em;
	}

	/* Footer */
	.panel-footer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 8px 16px;
		background: var(--surface-700);
		border-top: 1px solid var(--surface-border);
		flex-shrink: 0;
	}

	.line-jump {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.line-jump label {
		font-size: 10px;
		color: var(--text-muted);
		letter-spacing: 0.05em;
	}

	.line-jump input {
		width: 60px;
		padding: 4px 8px;
		background: var(--surface-800);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-primary);
		font-family: inherit;
		font-size: 11px;
		text-align: center;
	}

	.line-jump input:focus {
		outline: none;
		border-color: var(--amber-600);
		box-shadow: 0 0 8px var(--tint-focus);
	}

	.line-jump input::placeholder { color: var(--text-muted); }

	.current-line {
		font-size: 10px;
		color: var(--amber-400);
		font-weight: 600;
		letter-spacing: 0.05em;
	}

	/* Mobile responsive */
	@media (max-width: 639px) {
		.file-viewer-panel {
			width: 100% !important;
			min-width: 100%;
			max-width: 100%;
		}

		.resize-handle { display: none; }
		.panel-header { padding: 14px 16px; gap: 10px; }
		.file-info { gap: 12px; }
		.file-icon { font-size: 20px; }
		.filename { font-size: 14px; }
		.filepath { font-size: 11px; }
		.header-actions { gap: 6px; }
		.language-badge { display: none; }
		.line-count { display: none; }

		.action-btn,
		.close-btn { width: 40px; height: 40px; }

		.action-btn svg,
		.close-btn svg { width: 20px; height: 20px; }

		.panel-footer { padding: 10px 16px; }
		.line-jump { gap: 10px; }
		.line-jump label { font-size: 11px; }
		.line-jump input { width: 70px; padding: 8px 10px; font-size: 14px; min-height: 40px; }
		.current-line { font-size: 11px; }
	}
</style>
