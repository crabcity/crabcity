<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    label: string;
    description?: string;
    dirty: boolean;
    overridden: boolean;
    children: Snippet;
  }

  let { label, description, dirty, overridden, children }: Props = $props();
</script>

<div class="field-row">
  <div class="field-left">
    <span class="dirty-dot" class:visible={dirty}></span>
    <div class="field-info">
      <span class="field-label">{label}</span>
      {#if description}
        <span class="field-desc">{description}</span>
      {/if}
    </div>
  </div>
  <div class="field-right">
    <div class="field-control">
      {@render children()}
    </div>
    <span class="provenance-badge" class:ephemeral={overridden}>
      {overridden ? 'EPHEMERAL' : 'SAVED'}
    </span>
  </div>
</div>

<style>
  .field-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 0;
    gap: 12px;
  }

  .field-left {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .dirty-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--amber-500);
    flex-shrink: 0;
    opacity: 0;
    transition: opacity 0.15s ease, box-shadow 0.15s ease;
  }

  .dirty-dot.visible {
    opacity: 1;
    box-shadow: 0 0 4px var(--amber-glow);
  }

  .field-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .field-label {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-secondary);
    letter-spacing: 0.03em;
  }

  .field-desc {
    font-size: 10px;
    color: var(--text-muted);
    letter-spacing: 0.02em;
  }

  .field-right {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .field-control {
    display: flex;
    align-items: center;
  }

  .provenance-badge {
    font-size: 9px;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    padding: 1px 5px;
    border-radius: 3px;
    background: rgba(148, 163, 184, 0.1);
    color: var(--text-muted);
    flex-shrink: 0;
    min-width: 56px;
    text-align: center;
  }

  .provenance-badge.ephemeral {
    background: rgba(245, 158, 11, 0.12);
    color: var(--amber-400);
  }
</style>
