<script lang="ts">
  import type { PaneContentKind, PaneContent } from '$lib/stores/layout';
  import { setPaneContent, defaultContentForKind } from '$lib/stores/layout';
  import { currentInstanceId, instances, createInstance } from '$lib/stores/instances';
  import { userSettings } from '$lib/stores/settings';

  interface Props {
    paneId: string;
    content: PaneContent & { kind: 'picker' };
  }

  let { paneId, content }: Props = $props();

  const options: { kind: PaneContentKind; label: string; icon: string }[] = [
    {
      kind: 'terminal',
      label: 'Terminal',
      icon: '<rect x="2" y="3" width="16" height="13" rx="1.5"/><polyline points="5 8 7.5 10.5 5 13"/><line x1="10" y1="13" x2="14" y2="13"/>'
    },
    {
      kind: 'conversation',
      label: 'Conversation',
      icon: '<path d="M3 4h14a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1h-4l-3 3v-3H3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z"/>'
    },
    {
      kind: 'file-explorer',
      label: 'Files',
      icon: '<path d="M2 5V4a1 1 0 0 1 1-1h5l2 2h7a1 1 0 0 1 1 1v1"/><rect x="2" y="5" width="16" height="11" rx="1"/>'
    },
    {
      kind: 'chat',
      label: 'Chat',
      icon: '<circle cx="10" cy="10" r="7.5"/><circle cx="6.5" cy="9.5" r="1"/><circle cx="10" cy="9.5" r="1"/><circle cx="13.5" cy="9.5" r="1"/>'
    },
    {
      kind: 'tasks',
      label: 'Tasks',
      icon: '<rect x="3" y="3" width="5" height="5" rx="0.5"/><rect x="3" y="12" width="5" height="5" rx="0.5"/><line x1="11" y1="5.5" x2="17" y2="5.5"/><line x1="11" y1="14.5" x2="17" y2="14.5"/>'
    },
    {
      kind: 'file-viewer',
      label: 'File Viewer',
      icon: '<path d="M5 2h8l4 4v12a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z"/><polyline points="12 2 12 6 17 6"/><line x1="7" y1="10" x2="14" y2="10"/><line x1="7" y1="13" x2="12" y2="13"/>'
    },
    {
      kind: 'git',
      label: 'Git',
      icon: '<circle cx="10" cy="4" r="2"/><circle cx="10" cy="16" r="2"/><circle cx="16" cy="10" r="2"/><line x1="10" y1="6" x2="10" y2="14"/><path d="M10 8c0 2 6 2 6 2"/>'
    },
    {
      kind: 'settings',
      label: 'Settings',
      icon: '<circle cx="10" cy="10" r="3"/><path d="M10 2v2m0 12v2M4.2 4.2l1.4 1.4m8.8 8.8l1.4 1.4M2 10h2m12 0h2M4.2 15.8l1.4-1.4m8.8-8.8l1.4-1.4"/>'
    }
  ];

  async function handleSelect(kind: PaneContentKind) {
    // Resolve context from picker's source pane, falling back to current instance
    const workingDir = content.sourceWorkingDir ?? null;
    // Find an instance in this directory for instance-bound pane kinds
    const instanceId = (() => {
      if (workingDir) {
        for (const inst of $instances.values()) {
          if (inst.working_dir === workingDir) return inst.id;
        }
      }
      return $currentInstanceId;
    })();
    if (kind === 'terminal') {
      // Auto-create a shell instance using the configured shell command
      const result = await createInstance({
        command: $userSettings.shellCommand || 'bash',
        working_dir: workingDir ?? undefined
      });
      if (result) {
        setPaneContent(paneId, { kind: 'terminal', instanceId: result.id });
      }
      return;
    }
    setPaneContent(paneId, defaultContentForKind(kind, instanceId, workingDir));
  }
</script>

<div class="picker">
  <div class="picker-inner">
    <h2 class="picker-title">Select Pane Type</h2>
    <div class="kind-grid">
      {#each options as opt}
        <button class="kind-card" onclick={() => handleSelect(opt.kind)}>
          <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" class="kind-icon">
            {@html opt.icon}
          </svg>
          <span class="kind-label">{opt.label}</span>
        </button>
      {/each}
    </div>
  </div>
</div>

<style>
  .picker {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: 1;
    min-height: 0;
  }

  .picker-inner {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    max-width: 320px;
    width: 100%;
    padding: 24px;
  }

  .picker-title {
    margin: 0;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--amber-500);
  }

  .kind-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
    width: 100%;
  }

  .kind-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 12px 8px;
    background: var(--surface-700);
    border: 1px solid var(--surface-border);
    border-radius: 4px;
    color: var(--text-secondary);
    font-family: inherit;
    cursor: pointer;
    transition: all 0.1s ease;
  }

  .kind-card:hover {
    border-color: var(--amber-600);
    background: var(--tint-hover);
  }

  .kind-card:hover .kind-icon {
    color: var(--amber-400);
  }

  .kind-card:hover .kind-label {
    color: var(--amber-400);
  }

  .kind-icon {
    width: 20px;
    height: 20px;
    color: var(--text-muted);
    transition: color 0.1s ease;
  }

  .kind-label {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-secondary);
    transition: color 0.1s ease;
  }
</style>
