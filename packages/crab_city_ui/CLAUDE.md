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

**Settings** (`settings.ts`): Layered persistence — localStorage for instant hydration, server (`/api/user/settings`) as source of truth for cross-device sync. On WebSocket connect, client fetches server settings and merges (server wins). Changes write to both localStorage and server (async PATCH), then broadcast to all clients via `UserSettingsUpdate`. UI-only keys (e.g. `drawerWidth`) skip server sync. Quick access via gear icon in sidebar; full settings via `settings` pane kind.

### Layout Architecture

The UI uses a **binary split pane** layout (tmux-inspired). Core data lives in `stores/layout.ts`:

- **`LayoutNode`** = `SplitNode | LeafNode` — recursive tree, immutable updates
- **`PaneContent`** = discriminated union tagged by `kind`:
  - `{ kind: 'terminal'; instanceId: string | null }` — terminal bound to an instance
  - `{ kind: 'conversation'; instanceId: string | null; viewMode: 'structured' | 'raw' }` — conversation view (structured = notebook cells, raw = terminal inside conversation chrome)
  - `{ kind: 'file-viewer'; filePath: string | null; lineNumber?: number }` — self-contained file viewer
  - `{ kind: 'file-explorer'; instanceId: string | null }` — file tree for instance's working_dir
  - `{ kind: 'chat'; scope: 'global' | string }` — chat panel (global or instance-scoped)
  - `{ kind: 'tasks'; instanceId: string | null }` — task panel
  - `{ kind: 'git'; instanceId: string | null }` — git diff/log view
  - `{ kind: 'settings' }` — settings panel (no instanceId)
- Helpers: `getPaneInstanceId(content)` extracts instanceId from any variant; `defaultContentForKind(kind, instanceId)` constructs default config
- Actions: `splitPane`, `closePane`, `focusPane`, `setSplitRatio`, `setPaneContent`, `setPaneViewMode`, `togglePaneViewMode`

Components in `src/lib/components/layout/`:
- **LayoutTree.svelte** — recursive renderer (split → flex + SplitHandle, leaf → PaneHost). CSS transitions on split/close (150ms, disabled during drag). On mobile with multiple panes, renders focused pane with a tab bar (instance names, status dots, close/add buttons).
- **PaneHost.svelte** — dispatches to content component by `kind`, passes explicit props from the discriminated union (no global fallback). Each pane owns its own `instanceId`/`filePath`/`scope`.
- **PaneChrome.svelte** — title bar with content type dropdown, instance selector (for instance-bound kinds), split/close buttons, status dot. File-viewer panes show filename; chat panes show scope label.
- **SplitHandle.svelte** — drag-to-resize between split children, keyboard accessible (`role="separator"`, arrow keys ±5%/±15%, Home=50%)
- **Pane\*.svelte** — thin wrappers (PaneTerminal, PaneConversation, PaneFileExplorer, PaneChat, PaneTasks, PaneFileViewer, PaneGit, PaneSettings). Each accepts explicit props from its union variant.

**PaneFileViewer** is self-contained — it fetches file content independently via `apiGet`, has its own loading/error/empty states, and does not read global file viewer state. Two file-viewer panes can show different files simultaneously.

**Embedded panel pattern**: FileExplorer, ChatPanel, TaskPanel, FileViewer accept an `embedded` prop. When `true`, they skip the `position: fixed` overlay chrome (backdrop, close button, resize handle) and render inline. Pane wrappers pass `embedded={true}`. In single-pane mode, overlays still work as before.

**Persistence**: Layout serializes to `localStorage` key `crab_city_layout` (schema version 3, debounced 300ms, flushed on `beforeunload`). Deserialization migrates legacy flat format (version 1→2) and adds `viewMode` to conversation panes (version 2→3). All layouts (including single-pane) are restored from persistence.

**Presets**: `applyPreset('single' | 'dev-split' | 'side-by-side')` — accessible from MainHeader.

**Terminal cap**: Max 6 terminal panes enforced in `splitPane()` and `setPaneContent()`. Hitting the cap shows a toast notification.

**Orphan cleanup**: When an instance is deleted, `pruneInstancePanes()` reassigns any panes referencing that instance to the current global instance (or null → empty state).

**Persistence hardening**: Deserialization validates schema version, content kinds, tree-pane consistency, and clamps split ratios to [0.15, 0.85]. Corrupt state falls back gracefully with `console.warn`.

**Toast notifications**: `stores/toasts.ts` provides `addToast(message, type?, duration?)`. Max 3 visible (FIFO). `ToastStack.svelte` renders fixed bottom-right with slide-up animation.

**Cross-view focus**: `currentInstanceId` is driven one-way from `focusedPaneInstanceId` via `driveCurrentInstanceId()` (set up in `setupLayoutSync()`). To change the current instance, always use `setFocusedInstance(id)` or `selectInstance(id)` — never write to `currentInstanceId` directly. `setFocusedInstance()` routes through the layout bridge: it finds a pane already showing the instance and focuses it, or binds the focused pane to the new instance (choosing `conversation` vs `terminal` pane kind based on `InstanceKind`). Terminal focus handoff uses per-pane `requestTerminalFocus(paneId)` / `consumeTerminalFocus(paneId)` in `layout.ts`. There is no global `showTerminal` store — `PaneContent` is the single source of truth for what each pane displays, including the `viewMode` on conversation panes.

### Project & Instance Hierarchy

Instances are grouped into **projects** client-side by `working_dir` (`stores/projects.ts`). A `Project` is purely derived — no server changes, no persistence. Projects appear/disappear as instances are created/destroyed.

- **`projects`** — derived store: groups `instanceList` by `working_dir`
- **`currentProject`** — derived from `currentInstanceId` → find project containing that instance

**Sidebar** (`Sidebar.svelte`): 48px vertical icon rail showing project abbreviations (2-letter circles). Active project has amber highlight. Bottom: new instance, theme toggle, avatar/logout.

**MainHeader** (`main-view/MainHeader.svelte`): Project control center with three zones:
- Left: project name + connection status
- Center: `FleetStrip.svelte` — adaptive fleet visualization using `ResizeObserver`. Rendering modes: `detail` (≥200px/cell: icon, name, state+duration, tool), `compact` (80-199px: icon, truncated name, LED), `column` (30-79px: colored bars with attention pips), `aggregate` (<30px: proportional bar + counts). Inbox summary text right-aligned. Expand chevron opens FleetPanel.
- Right: action buttons (layout presets, files, tasks, chat)

**FleetPanel** (`main-view/FleetPanel.svelte`): Expanded fleet control panel (replaces FleetDrawer). Three tiers sorted by attention:
- **Inbox**: items from `stores/inbox.ts` — `needs_input` (respond button), `completed_turn` (review/dismiss), `error` (dismiss). Auto-sorted by priority.
- **Active**: instances currently Thinking/Responding/ToolExecuting/Starting, with state + duration
- **Idle**: collapsible when >4, compact chip grid when collapsed
Search filter, keyboard nav (arrows/Enter/Escape), right-click → `InstancePopover.svelte`. Hidden on mobile.

**Instance state utility** (`utils/instance-state.ts`): `getStateInfo()` extracts display state (label, color, animation) from instance + claude state. Shared by Sidebar rail, FleetStrip, and FleetPanel. `InstanceKind` drives the kind icon (brain for Structured, terminal prompt for Unstructured).

**Inbox store** (`stores/inbox.ts`): Server-side inbox model — `inboxItems` (Map by instance_id), `inboxSorted` (priority-sorted), `inboxCount`. Pure utilities: `getAttentionLevel()` (critical/warning/active/idle/booting), `formatDuration()`. Browser notifications for `needs_input` events. WS messages: `InboxUpdate` (single upsert/delete), `InboxList` (initial load). HTTP: `POST /api/inbox/{id}/dismiss`.

### API Calls

Use `src/lib/utils/api.ts` for all HTTP requests. It handles auth headers, base URL resolution, and error normalization. Do not use `fetch` directly.

### Types

Domain types are defined in `src/lib/types.ts`. Keep them in sync with the Rust `models.rs` types. The JSON serialization from the server is the contract.

**`InstanceKind`** — discriminated union (`{ type: 'Structured'; provider: string } | { type: 'Unstructured'; label?: string | null }`). Computed by the backend at creation time, sent in the wire protocol on every `Instance` payload. Frontend checks `inst.kind.type === 'Structured'` instead of `command.includes('claude')`. The `isClaudeInstance` derived store wraps this check for the current instance.

### Styling

Follow the amber phosphor CRT design system documented in `BRAND_BOOK.md`:
- Use CSS custom properties (`--amber-*`, `--surface-*`, `--text-*`) — never hardcode colors
- Monospace typography only (JetBrains Mono stack)
- Uppercase labels with letter-spacing for industrial feel
- CRT scanlines and glow effects are part of the aesthetic, not decoration

### File Organization

Components with subcomponents live in directories (e.g. `file-explorer/`, `chat-panel/`, `layout/`). Top-level components in `src/lib/components/` are the main views. The `layout/` directory contains the pane tree renderer and all pane wrapper components.

### Formatting

Prettier formats all TS and Svelte files. Config is in `.prettierrc` (spaces, not tabs; single quotes; 120 print width). Run `pnpm format` or `bazel run //tools/format`. CI checks formatting via `bazel test //tools/format:format_test`.

### Testing

Test files sit alongside source: `foo.test.ts` next to `foo.ts`. Run with `pnpm test`.
