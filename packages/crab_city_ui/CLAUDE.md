# crab_city_ui — CLAUDE.md

## Stack

- **SvelteKit** 2.x with **Svelte 5** (runes, `$state`, `$derived`, `$effect`)
- **TypeScript** 5.7
- **xterm.js** 6.0 for terminal emulation
- **marked** + **highlight.js** for markdown/code rendering
- **d3** for the conversation minimap
- **Jest** for testing
- **pnpm** as package manager

## Conventions

### State Management

All shared state lives in `src/lib/stores/`. Stores use Svelte 5 runes (`$state`, `$derived`). The WebSocket store (`websocket.ts`) is the backbone — it manages the connection and dispatches incoming `ServerMessage` payloads via `ws-handlers.ts` to domain-specific stores.

Stores must handle WebSocket broadcasts **idempotently** (upsert by ID, not blind append) because the originating client receives both its HTTP response and its own broadcast echo.

**localStorage-backed stores** (`settings.ts`, `drafts.ts`) persist client-only preferences and draft input across page reloads. Keep business logic in pure utility modules (`utils/draft-map.ts`) and limit the store to wiring reactivity + persistence. Debounce writes; flush synchronously on `beforeunload`.

### Layout Architecture

The UI uses a **binary split pane** layout (tmux-inspired). Core data lives in `stores/layout.ts`:

- **`LayoutNode`** = `SplitNode | LeafNode` — recursive tree, immutable updates
- **`PaneState`** = `{ id, content: { kind, instanceId? } }` — each pane's view
- **`PaneContentKind`** = `terminal | conversation | file-explorer | chat | tasks | file-viewer | git`
- Actions: `splitPane`, `closePane`, `focusPane`, `setSplitRatio`, `setPaneContent`

Components in `src/lib/components/layout/`:
- **LayoutTree.svelte** — recursive renderer (split → flex + SplitHandle, leaf → PaneHost). CSS transitions on split/close (150ms, disabled during drag). On mobile with multiple panes, renders focused pane with a tab bar (instance names, status dots, close/add buttons).
- **PaneHost.svelte** — dispatches to content component by `kind`, captures focus, shows empty state when no instance selected
- **PaneChrome.svelte** — title bar (content type dropdown, split/close buttons, status dot: purple=thinking, amber=responding/tool)
- **SplitHandle.svelte** — drag-to-resize between split children, keyboard accessible (`role="separator"`, arrow keys ±5%/±15%, Home=50%)
- **Pane\*.svelte** — thin wrappers (PaneTerminal, PaneConversation, PaneFileExplorer, PaneChat, PaneTasks, PaneFileViewer, PaneGit)

**Embedded panel pattern**: FileExplorer, ChatPanel, TaskPanel, FileViewer accept an `embedded` prop. When `true`, they skip the `position: fixed` overlay chrome (backdrop, close button, resize handle) and render inline. Pane wrappers pass `embedded={true}`. In single-pane mode, overlays still work as before.

**Persistence**: Layout serializes to `localStorage` key `crab_city_layout` (debounced 300ms, flushed on `beforeunload`). Only multi-pane layouts are restored; single-pane syncs with `showTerminal`.

**Presets**: `applyPreset('single' | 'dev-split' | 'side-by-side')` — accessible from MainHeader.

**Terminal cap**: Max 6 terminal panes enforced in `splitPane()` and `setPaneContent()`. Hitting the cap shows a toast notification.

**Orphan cleanup**: When an instance is deleted, `pruneInstancePanes()` reassigns any panes referencing that instance to the current global instance (or null → empty state).

**Persistence hardening**: Deserialization validates schema version, content kinds, tree-pane consistency, and clamps split ratios to [0.15, 0.85]. Corrupt state falls back gracefully with `console.warn`.

**Toast notifications**: `stores/toasts.ts` provides `addToast(message, type?, duration?)`. Max 3 visible (FIFO). `ToastStack.svelte` renders fixed bottom-right with slide-up animation.

**Cross-view focus**: When focus changes between panes, `focusedPaneInstanceId` syncs to `currentInstanceId` so the sidebar highlights the correct instance. The flag-and-consume pattern in `stores/instances.ts` still handles focus handoff on view switch — see [docs/web-terminal.md](../../docs/web-terminal.md#view-switching-and-focus-handoff).

### API Calls

Use `src/lib/utils/api.ts` for all HTTP requests. It handles auth headers, base URL resolution, and error normalization. Do not use `fetch` directly.

### Types

Domain types are defined in `src/lib/types.ts`. Keep them in sync with the Rust `models.rs` types. The JSON serialization from the server is the contract.

### Styling

Follow the amber phosphor CRT design system documented in `BRAND_BOOK.md`:
- Use CSS custom properties (`--amber-*`, `--surface-*`, `--text-*`) — never hardcode colors
- Monospace typography only (JetBrains Mono stack)
- Uppercase labels with letter-spacing for industrial feel
- CRT scanlines and glow effects are part of the aesthetic, not decoration

### File Organization

Components with subcomponents live in directories (e.g. `file-explorer/`, `chat-panel/`, `layout/`). Top-level components in `src/lib/components/` are the main views. The `layout/` directory contains the pane tree renderer and all pane wrapper components.

### Testing

Test files sit alongside source: `foo.test.ts` next to `foo.ts`. Run with `pnpm test`.
