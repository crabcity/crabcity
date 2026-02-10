<script lang="ts">
	import {
		isExplorerOpen,
		closeExplorer,
		fileViewerState,
		openFileDiffLoading,
		openFileDiff,
		setDiffData,
		setDiffError
	} from '$lib/stores/files';
	import { currentInstance } from '$lib/stores/instances';
	import { isDesktop } from '$lib/stores/ui';
	import { diffEngine } from '$lib/stores/settings';
	import {
		isGitOpen, openGitTab, closeGitTab, gitTab, gitDiff, gitError,
		statusCounts,
		selectedCommitHash, selectedBranchName, branchLogPreview, branchDiff, branchDiffBase, branchDiffMode, logBranchFilter,
		fetchGitLog, fetchGitBranches, fetchGitStatus, fetchGitDiff,
		startGitRefresh, stopGitRefresh,
		type GitFileStatus, type GitDiffFile
	} from '$lib/stores/git';
	import GitLog from './file-explorer/GitLog.svelte';
	import GitBranches from './file-explorer/GitBranches.svelte';
	import GitStatus from './file-explorer/GitStatus.svelte';
	import FileBrowser from './file-explorer/FileBrowser.svelte';

	// Panel width with resize support
	let panelWidth = $state(320);
	let isResizing = $state(false);
	let startX = $state(0);
	let startWidth = $state(0);

	// Git log pagination
	let logOffset = $state(0);

	// --- Git data loading effects ---

	// Fetch git data when git tab is active
	$effect(() => {
		const open = $isGitOpen;
		const instance = $currentInstance;
		const tab = $gitTab;
		const branchFilter = $logBranchFilter;
		if (open && instance) {
			if (tab === 'log') { logOffset = 0; fetchGitLog(instance.id, branchFilter ? { branch: branchFilter } : undefined); }
			else if (tab === 'branches') fetchGitBranches(instance.id);
			else if (tab === 'status') fetchGitStatus(instance.id);
			startGitRefresh(instance.id);
		} else {
			stopGitRefresh();
		}
	});

	// Clear branch expansion when leaving the branches tab
	$effect(() => {
		const tab = $gitTab;
		if (tab !== 'branches') {
			selectedBranchName.set(null);
			branchLogPreview.set([]);
			branchDiff.set(null);
			branchDiffBase.set(null);
		}
	});

	// Always fetch git status for file badges (even in Files mode)
	$effect(() => {
		const open = $isExplorerOpen;
		const instance = $currentInstance;
		if (open && instance) {
			fetchGitStatus(instance.id);
		}
	});

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
		const deltaX = e.clientX - startX;
		const newWidth = Math.max(240, Math.min(startWidth + deltaX, window.innerWidth * 0.5));
		panelWidth = newWidth;
	}

	function stopResize() {
		if (isResizing) {
			isResizing = false;
			document.body.style.cursor = '';
			document.body.style.userSelect = '';
		}
	}

	// Handle clicking a file in the git status view — open its diff
	function handleStatusFileClick(file: GitFileStatus) {
		const instance = $currentInstance;
		if (!instance) return;
		const targetPath = file.path;
		openFileDiffLoading(targetPath);
		fetchGitDiff(instance.id, undefined, targetPath, $diffEngine).then(() => {
			const currentState = $fileViewerState;
			if (currentState.filePath !== targetPath) return;
			const diff = $gitDiff;
			if (diff && diff.files.length > 0) {
				setDiffData(diff.files[0]);
			} else {
				setDiffError('No changes found');
			}
		}).catch(() => {
			const currentState = $fileViewerState;
			if (currentState.filePath !== targetPath) return;
			setDiffError();
		});
	}

	// Handle clicking a file in an expanded commit diff
	function handleCommitFileClick(diffFile: GitDiffFile) {
		openFileDiff(diffFile.path, diffFile, $selectedCommitHash ?? undefined);
	}

	// Handle clicking a file in the branch diff expansion
	function handleBranchDiffFileClick(diffFile: GitDiffFile) {
		const instance = $currentInstance;
		const branchName = $selectedBranchName;
		const base = $branchDiffBase;
		if (!instance || !branchName || !base) return;
		const targetPath = diffFile.path;
		const mode = $branchDiffMode;
		openFileDiffLoading(targetPath);
		fetchGitDiff(instance.id, undefined, targetPath, $diffEngine, {
			base,
			head: branchName,
			diffMode: mode,
		}).then(() => {
			const currentState = $fileViewerState;
			if (currentState.filePath !== targetPath) return;
			const diff = $gitDiff;
			if (diff && diff.files.length > 0) {
				setDiffData(diff.files[0]);
			} else {
				setDiffError('No changes found');
			}
		}).catch(() => {
			const currentState = $fileViewerState;
			if (currentState.filePath !== targetPath) return;
			setDiffError();
		});
	}

	// Load more commits
	function loadMoreCommits() {
		const instance = $currentInstance;
		if (!instance) return;
		logOffset += 50;
		const branchFilter = $logBranchFilter;
		fetchGitLog(instance.id, { offset: logOffset, ...(branchFilter ? { branch: branchFilter } : {}) });
	}

	// Relative time helper
	function relativeTime(unixSeconds: number): string {
		const diff = Math.floor(Date.now() / 1000) - unixSeconds;
		if (diff < 60) return 'just now';
		if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
		if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
		if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`;
		return `${Math.floor(diff / 604800)}w ago`;
	}
</script>

<svelte:window onmousemove={handleMouseMove} onmouseup={stopResize} />

{#if $isExplorerOpen}
	<!-- Mobile backdrop -->
	{#if !$isDesktop}
		<button class="explorer-backdrop" onclick={closeExplorer} aria-label="Close file explorer"></button>
	{/if}

	<!-- Panel -->
	<aside class="file-explorer-panel" style="width: {$isDesktop ? panelWidth : undefined}px">
		<!-- Header -->
		<header class="panel-header">
			<div class="header-title">
				<span class="folder-icon">{$isGitOpen ? '' : ''}</span>
				<span class="title-text">{$isGitOpen ? 'Git' : 'Files'}</span>
			</div>
			<button
				class="close-btn"
				onclick={closeExplorer}
				aria-label="Close file explorer"
			>
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<line x1="18" y1="6" x2="6" y2="18"></line>
					<line x1="6" y1="6" x2="18" y2="18"></line>
				</svg>
			</button>
		</header>

		<!-- Mode tabs: Files | Git -->
		<div class="explorer-tabs">
			<button class="explorer-tab" class:active={!$isGitOpen} onclick={() => closeGitTab()}>
				Files
			</button>
			<button class="explorer-tab" class:active={$isGitOpen} onclick={() => openGitTab()}>
				Git
				{#if $statusCounts.total > 0}
					<span class="git-tab-badge">{$statusCounts.total}</span>
				{/if}
			</button>
		</div>

		{#if $isGitOpen}
			<!-- Git mode -->
			<div class="git-subtabs">
				<button class="git-subtab" class:active={$gitTab === 'log'} onclick={() => gitTab.set('log')}>Log</button>
				<button class="git-subtab" class:active={$gitTab === 'branches'} onclick={() => gitTab.set('branches')}>Branches</button>
				<button class="git-subtab" class:active={$gitTab === 'status'} onclick={() => gitTab.set('status')}>
					Status
					{#if $statusCounts.total > 0}
						<span class="subtab-count">{$statusCounts.total}</span>
					{/if}
				</button>
			</div>

			<div class="git-content">
				{#if $gitError}
					<div class="error-state">
						<span class="error-icon">&#x26A0;&#xFE0F;</span>
						<span class="error-text">{$gitError}</span>
					</div>
				{:else if $gitTab === 'log'}
					<GitLog onCommitFileClick={handleCommitFileClick} onLoadMore={loadMoreCommits} {relativeTime} />
				{:else if $gitTab === 'branches'}
					<GitBranches onBranchDiffFileClick={handleBranchDiffFileClick} {relativeTime} />
				{:else if $gitTab === 'status'}
					<GitStatus onStatusFileClick={handleStatusFileClick} />
				{/if}
			</div>
		{:else}
			<!-- Files mode -->
			<FileBrowser />
		{/if}

		<!-- Resize handle -->
		<button
			class="resize-handle"
			onmousedown={startResize}
			aria-label="Resize panel"
		></button>
	</aside>
{/if}

<style>
	/* Mobile backdrop */
	.explorer-backdrop {
		position: fixed;
		inset: 0;
		background: var(--backdrop);
		z-index: 89;
		border: none;
		cursor: default;
		animation: fadeIn 0.15s ease-out;
	}

	@keyframes fadeIn {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	.file-explorer-panel {
		position: fixed;
		top: 0;
		left: var(--sidebar-width, 260px);
		bottom: 0;
		display: flex;
		flex-direction: column;
		background: var(--surface-900);
		border-right: 1px solid var(--surface-border);
		z-index: 90;
		min-width: 240px;
		max-width: 50vw;
		box-shadow: var(--shadow-panel);

		/* Slide-in animation */
		animation: slideInLeft 0.2s ease-out;
	}

	@keyframes slideInLeft {
		from {
			transform: translateX(-20%);
			opacity: 0;
		}
		to {
			transform: translateX(0);
			opacity: 1;
		}
	}

	/* Header */
	.panel-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 12px 14px;
		background: linear-gradient(180deg, var(--surface-700) 0%, var(--surface-800) 100%);
		border-bottom: 1px solid var(--surface-border);
		flex-shrink: 0;
	}

	.header-title {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.folder-icon {
		font-size: 16px;
	}

	.title-text {
		font-size: 12px;
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--amber-400);
		text-shadow: var(--emphasis);
	}

	.close-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
		background: none;
		border: none;
		color: var(--text-muted);
		cursor: pointer;
		border-radius: 3px;
		transition: all 0.15s ease;
	}

	.close-btn:hover {
		background: var(--surface-600);
		color: var(--text-primary);
	}

	.close-btn svg {
		width: 14px;
		height: 14px;
	}

	/* Explorer mode tabs: Files | Git */
	.explorer-tabs {
		display: flex;
		background: var(--surface-800);
		border-bottom: 1px solid var(--surface-border);
		flex-shrink: 0;
	}

	.explorer-tab {
		flex: 1;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		padding: 8px 12px;
		background: none;
		border: none;
		border-bottom: 2px solid transparent;
		font-family: inherit;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-muted);
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.explorer-tab:hover {
		color: var(--text-secondary);
		background: var(--tint-subtle);
	}

	.explorer-tab.active {
		color: var(--amber-400);
		border-bottom-color: var(--amber-500);
		text-shadow: var(--emphasis);
	}

	.git-tab-badge {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		min-width: 16px;
		height: 16px;
		padding: 0 4px;
		font-size: 9px;
		font-weight: 700;
		line-height: 1;
		border-radius: 8px;
		background: var(--amber-500);
		color: var(--btn-primary-text);
	}

	/* Git sub-tabs: Log | Branches | Status */
	.git-subtabs {
		display: flex;
		gap: 2px;
		padding: 4px 8px;
		background: var(--surface-800);
		border-bottom: 1px solid var(--surface-border);
		flex-shrink: 0;
	}

	.git-subtab {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 4px 10px;
		background: none;
		border: none;
		border-radius: 3px;
		font-family: inherit;
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.05em;
		color: var(--text-muted);
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.git-subtab:hover {
		background: var(--surface-700);
		color: var(--text-secondary);
	}

	.git-subtab.active {
		background: var(--surface-600);
		color: var(--amber-400);
	}

	.subtab-count {
		font-size: 9px;
		font-weight: 700;
		color: var(--amber-500);
	}

	/* Git content area */
	.git-content {
		flex: 1;
		overflow-y: auto;
		overflow-x: hidden;
		padding: 2px 0;
	}

	.git-content::-webkit-scrollbar {
		width: 6px;
	}

	.git-content::-webkit-scrollbar-track {
		background: var(--surface-900);
	}

	.git-content::-webkit-scrollbar-thumb {
		background: var(--surface-400);
		border-radius: 3px;
	}

	.git-content::-webkit-scrollbar-thumb:hover {
		background: var(--amber-600);
	}

	/* Git error state */
	.error-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		padding: 32px 16px;
		gap: 12px;
		color: var(--text-muted);
	}

	.error-icon {
		font-size: 24px;
		opacity: 0.5;
	}

	.error-text {
		font-size: 11px;
		text-align: center;
	}

	/* Resize handle */
	.resize-handle {
		position: absolute;
		right: -4px;
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

	/* Tablet - sidebar hidden, panel starts at left edge */
	@media (max-width: 1023px) {
		.file-explorer-panel {
			left: 0;
			width: 85vw !important;
			min-width: 280px;
			max-width: 400px;
		}
	}

	/* Mobile - full screen overlay */
	@media (max-width: 639px) {
		.file-explorer-panel {
			left: 0;
			width: 100% !important;
			min-width: 100%;
			max-width: 100%;
			border-right: none;
		}

		.resize-handle {
			display: none;
		}

		.panel-header {
			padding: 14px 16px;
		}

		.header-title {
			gap: 10px;
		}

		.folder-icon {
			font-size: 18px;
		}

		.title-text {
			font-size: 13px;
		}

		.close-btn {
			width: 36px;
			height: 36px;
		}

		.close-btn svg {
			width: 18px;
			height: 18px;
		}
	}
</style>
