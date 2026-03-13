<script lang="ts">
	import { onDestroy } from 'svelte';
	import MainHeader from './main-view/MainHeader.svelte';
	import LayoutTree from './layout/LayoutTree.svelte';
	import { currentInstance, currentInstanceId, isClaudeInstance, showTerminal, setTerminalMode, initTerminalModeFromUrl, initViewStateFromUrl } from '$lib/stores/instances';
	import { connect, disconnect } from '$lib/stores/websocket';
	import { isActive } from '$lib/stores/claude';
	import { currentVerb } from '$lib/stores/activity';
	import { openExplorer, fetchFileContent, openFileFromTool, openFileDiffLoading, setDiffData, setDiffError } from '$lib/stores/files';
	import { openGitTab, fetchGitDiff, gitDiff } from '$lib/stores/git';
	import { diffEngine } from '$lib/stores/settings';
	import { layoutRoot, setupLayoutSync, setupLayoutPersistence, tryRestoreLayout } from '$lib/stores/layout';
	import { get } from 'svelte/store';

	// Keep layout store in sync with showTerminal/currentInstanceId (Phase 1)
	setupLayoutSync();
	// Persist layout to localStorage and restore on load
	setupLayoutPersistence();
	tryRestoreLayout();

	let lastInstanceId: string | null = null;
	let hasInitializedFromUrl = false;

	// React to instance changes - connect to selected instance
	$effect(() => {
		const instanceId = $currentInstanceId;

		if (instanceId !== lastInstanceId) {
			lastInstanceId = instanceId;

			if (instanceId) {
				connect(instanceId);
				if (!hasInitializedFromUrl) {
					hasInitializedFromUrl = true;
					const urlTerminalMode = initTerminalModeFromUrl();
					const url = new URL(window.location.href);
					if (url.searchParams.has('terminal')) {
						setTerminalMode(urlTerminalMode);
					} else {
						setTerminalMode(!$isClaudeInstance);
					}

					const viewState = initViewStateFromUrl();
					if (viewState.explorer) {
						openExplorer();
						if (viewState.explorer === 'git') {
							openGitTab();
						}
					}
					if (viewState.file) {
						const filePath = viewState.file;
						if (viewState.view === 'diff') {
							openFileDiffLoading(filePath, viewState.commit);
							fetchGitDiff(instanceId, viewState.commit, filePath, get(diffEngine))
								.then(() => {
									const diff = get(gitDiff);
									if (diff && diff.files.length > 0) {
										setDiffData(diff.files[0]);
									} else {
										setDiffError('No changes found');
									}
								})
								.catch(() => setDiffError('Failed to load diff'));
						} else {
							const lineNum = viewState.line;
							fetchFileContent(filePath)
								.then((content) => {
									openFileFromTool(filePath, content, lineNum);
								})
								.catch((err) => {
									console.error('Failed to restore file from URL:', err);
								});
						}
					}
				}
			} else {
				disconnect();
			}
		}
	});

	onDestroy(() => {
		disconnect();
	});

	// Update browser tab title to reflect current instance and activity
	$effect(() => {
		const instance = $currentInstance;
		if (instance) {
			const displayName = instance.custom_name ?? instance.name;
			if ($isActive) {
				document.title = `${$currentVerb}... | ${displayName}`;
			} else {
				document.title = displayName;
			}
		} else {
			document.title = 'Crab City';
		}
	});
</script>

<main class="main-view">
	<MainHeader />
	<div class="content">
		<LayoutTree node={$layoutRoot} depth={0} />
	</div>
</main>

<style>
	.main-view {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-width: 0;
		background: var(--surface-800);
	}

	.content {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}
</style>
