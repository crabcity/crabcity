<script lang="ts">
  import { apiGet } from '$lib/utils/api';
  import { currentInstance } from '$lib/stores/instances';
  import { setPaneContent } from '$lib/stores/layout';
  import { openExplorerPicker } from '$lib/stores/files';
  import SourceView from '../file-viewer/SourceView.svelte';

  interface Props {
    filePath: string | null;
    lineNumber?: number;
    paneId: string;
  }

  let { filePath, lineNumber, paneId }: Props = $props();

  let content = $state<string | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  // Detect language from file extension
  const language = $derived.by(() => {
    if (!filePath) return '';
    const ext = filePath.split('.').pop()?.toLowerCase() ?? '';
    const map: Record<string, string> = {
      ts: 'typescript',
      tsx: 'tsx',
      js: 'javascript',
      jsx: 'jsx',
      rs: 'rust',
      py: 'python',
      go: 'go',
      rb: 'ruby',
      java: 'java',
      kt: 'kotlin',
      swift: 'swift',
      css: 'css',
      scss: 'scss',
      html: 'html',
      svelte: 'svelte',
      json: 'json',
      yaml: 'yaml',
      yml: 'yaml',
      toml: 'toml',
      md: 'markdown',
      markdown: 'markdown',
      sh: 'bash',
      bash: 'bash',
      zsh: 'bash',
      sql: 'sql',
      graphql: 'graphql',
      c: 'c',
      cpp: 'cpp',
      h: 'c',
      hpp: 'cpp',
      xml: 'xml',
      svg: 'xml'
    };
    return map[ext] ?? ext;
  });

  const filename = $derived(filePath?.split('/').pop() ?? 'File');

  // Fetch file content when filePath or instance changes
  $effect(() => {
    const path = filePath;
    const inst = $currentInstance;

    if (!path) {
      content = null;
      error = null;
      loading = false;
      return;
    }

    if (!inst) {
      // Instance not yet resolved — stay in loading state; the effect
      // will re-run when effectiveInstanceId settles.
      loading = true;
      error = null;
      return;
    }

    loading = true;
    error = null;
    content = null;

    apiGet<{ content: string }>(`/api/instances/${inst.id}/files/content?path=${encodeURIComponent(path)}`)
      .then((response) => {
        // Guard against stale response
        if (filePath !== path) return;
        content = response.content;
        loading = false;
      })
      .catch((err) => {
        if (filePath !== path) return;
        const msg = err?.message ?? '';
        if (msg.includes('403')) {
          error = 'File is outside the project directory';
        } else if (msg.includes('404')) {
          error = 'File not found';
        } else {
          error = msg || 'Failed to load file';
        }
        loading = false;
      });
  });

  const isMarkdown = $derived(language === 'markdown' || language === 'md');
  const lineCount = $derived(content?.split('\n').length ?? 0);

  let sourceViewRef: SourceView | undefined = $state();

  async function copyContent() {
    if (!content) return;
    try {
      await navigator.clipboard.writeText(content);
    } catch {
      // fallback
    }
  }

  function browseFiles() {
    openExplorerPicker((path) => {
      // Read $currentInstance at selection time, not at open time,
      // so we pick up the correct instance even if focus shifted.
      const inst = $currentInstance;
      setPaneContent(paneId, {
        kind: 'file-viewer',
        filePath: path,
        workingDir: inst?.working_dir ?? null
      });
    });
  }
</script>

<div class="pane-file-viewer">
  {#if !filePath}
    <button class="empty-state" onclick={browseFiles}>
      <svg class="empty-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
        <polyline points="14 2 14 8 20 8" />
      </svg>
      <span class="empty-label">No file selected</span>
      <span class="empty-hint">Click to browse files</span>
    </button>
  {:else if loading}
    <div class="loading-state">
      <div class="spinner"></div>
      <span>{filename}</span>
    </div>
  {:else if error}
    <div class="error-state">
      <span class="error-label">Error</span>
      <span class="error-text">{error}</span>
    </div>
  {:else if content !== null}
    <div class="viewer-header">
      <button class="viewer-filename" onclick={browseFiles} title="Browse files">
        {filename}
      </button>
      {#if language}
        <span class="language-badge">{language}</span>
      {/if}
      <span class="line-count">{lineCount} lines</span>
      <div class="viewer-spacer"></div>
      {#if isMarkdown}
        <button
          class="viewer-btn"
          class:active={sourceViewRef?.getShowPreview()}
          onclick={() => sourceViewRef?.togglePreview()}
          title="Toggle preview"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path>
            <circle cx="12" cy="12" r="3"></circle>
          </svg>
        </button>
      {/if}
      <button class="viewer-btn" onclick={copyContent} title="Copy content">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
          <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
        </svg>
      </button>
    </div>
    <div class="viewer-content">
      <SourceView
        bind:this={sourceViewRef}
        {content}
        {language}
        lineNumber={lineNumber ?? null}
        isError={false}
        {isMarkdown}
      />
    </div>
  {/if}
</div>

<style>
  .pane-file-viewer {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    flex: 1;
    gap: 8px;
    color: var(--text-muted);
    background: none;
    border: none;
    width: 100%;
    cursor: pointer;
    font-family: inherit;
    transition: color 0.15s ease;
  }

  .empty-state:hover {
    color: var(--text-secondary);
  }

  .empty-state:hover .empty-icon {
    opacity: 0.6;
  }

  .empty-icon {
    width: 32px;
    height: 32px;
    opacity: 0.4;
    margin-bottom: 4px;
  }

  .empty-label {
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .empty-hint {
    font-size: 10px;
    opacity: 0.6;
  }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    flex: 1;
    gap: 12px;
    color: var(--text-muted);
    font-size: 11px;
    letter-spacing: 0.05em;
  }

  .spinner {
    width: 20px;
    height: 20px;
    border: 2px solid var(--surface-border);
    border-top-color: var(--accent-400);
    border-radius: 50%;
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .error-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    flex: 1;
    gap: 6px;
  }

  .error-label {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--status-red);
  }

  .error-text {
    font-size: 10px;
    color: var(--text-muted);
  }

  .viewer-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    background: var(--surface-700);
    border-bottom: 1px solid var(--surface-border);
    flex-shrink: 0;
  }

  .viewer-filename {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.05em;
    color: var(--accent-400);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    background: none;
    border: none;
    padding: 2px 4px;
    margin: -2px -4px;
    border-radius: 3px;
    cursor: pointer;
    font-family: inherit;
    text-align: left;
    transition: background 0.15s ease;
  }

  .viewer-filename:hover {
    background: var(--tint-hover);
  }

  .language-badge {
    padding: 1px 5px;
    background: var(--surface-600);
    border: 1px solid var(--surface-border);
    border-radius: 3px;
    font-size: 8px;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--thinking-400);
    flex-shrink: 0;
  }

  .line-count {
    font-size: 9px;
    color: var(--text-muted);
    letter-spacing: 0.05em;
    flex-shrink: 0;
  }

  .viewer-spacer {
    flex: 1;
  }

  .viewer-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    background: transparent;
    border: none;
    border-radius: 2px;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.1s ease;
    flex-shrink: 0;
  }

  .viewer-btn:hover {
    background: var(--tint-hover);
    color: var(--text-secondary);
  }

  .viewer-btn.active {
    color: var(--accent-400);
  }

  .viewer-btn svg {
    width: 12px;
    height: 12px;
  }

  .viewer-content {
    flex: 1;
    min-height: 0;
    overflow: auto;
    background: var(--surface-800);
  }

  .viewer-content::-webkit-scrollbar {
    width: 6px;
    height: 6px;
  }
  .viewer-content::-webkit-scrollbar-track {
    background: var(--surface-900);
  }
  .viewer-content::-webkit-scrollbar-thumb {
    background: var(--surface-400);
    border-radius: 3px;
  }
  .viewer-content::-webkit-scrollbar-thumb:hover {
    background: var(--accent-600);
  }
</style>
