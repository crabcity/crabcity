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

Components with subcomponents live in directories (e.g. `file-explorer/`, `chat-panel/`). Top-level components in `src/lib/components/` are the main views.

### Testing

Test files sit alongside source: `foo.test.ts` next to `foo.ts`. Run with `pnpm test`.
