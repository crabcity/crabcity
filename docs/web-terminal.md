# Web Terminal

The web UI renders PTY output via [xterm.js](https://xtermjs.org/). This covers the client-side terminal: lifecycle, data flow, view switching, locking, and dimension negotiation.

Server-side multiplexing: [architecture.md#terminal-multiplexing](architecture.md#terminal-multiplexing).

## File Map

| File | Role |
|------|------|
| `components/Terminal.svelte` | xterm.js lifecycle, input/output, resize |
| `stores/terminal.ts` | Per-instance output buffer (producer/consumer) |
| `stores/layout.ts` | View mode (`PaneContent.viewMode`), per-pane focus handoff |
| `stores/terminalLock.ts` | Multi-user input lock (mirrors server) |
| `stores/websocket.ts` | Send helpers: `sendInput`, `sendResize`, `sendTerminalVisible/Hidden` |
| `stores/ws-handlers.ts` | Receive dispatch: `Output` → buffer, `OutputHistory` → buffer+clear |

## Lifecycle

Terminal and ConversationView are `{#if}`/`{:else}` branches in PaneConversation — they never coexist within a single pane. Each view switch is a full mount/unmount cycle. The `viewMode` field on the conversation `PaneContent` (`'structured'` or `'raw'`) determines which branch renders.

### Mount

```
onMount → initTerminal() [async]
  1. Capture mountedInstanceId (before any await — survives instance switches)
  2. Wait for terminalEl bind (tick + retry)
  3. Dynamic-import xterm.js + addons (keeps them out of initial bundle)
  4. terminal.open(terminalEl)
  5. requestAnimationFrame:
     - fitAddon.fit()
     - isReady = true  ← triggers $effect hooks (focus handoff, output subscription)
     - sendTerminalVisible(rows, cols)
```

### Teardown

`onDestroy` sends `TerminalHidden(mountedInstanceId)`, unsubscribes stores, disconnects ResizeObserver, disposes xterm.

## Data Flow

### Output: server → screen

```
PTY → WebSocket → ws-handlers.ts → stores/terminal.ts (buffer) → Terminal.svelte → xterm.write()
```

The buffer decouples producers from consumers. Output arrives even when Terminal isn't mounted (user is in ConversationView). Buffer is per-instance, FIFO-capped at 10k chunks (~1MB). On instance switch, `writeTerminalHistory` sets `shouldClear` so the terminal resets before replaying.

Auto-scroll: viewport stays put if user has scrolled up; auto-scrolls only if already at bottom.

### Input: keyboard → PTY

```
xterm.onData → lock check → sendInput(data) → WebSocket → server → PTY
```

## View Switching and Focus Handoff

**Problem:** When QuestionCard says "Switch to Terminal view", both the view switch and terminal focus must happen — but the producer (QuestionCard) is unmounting while the consumer (Terminal) hasn't mounted yet.

**Solution:** Per-pane flag-and-consume pattern in `stores/layout.ts`:

```
setPaneViewMode(paneId, 'raw')           Terminal.svelte
  ├─ content.viewMode = 'raw'              $effect watching isReady:
  └─ requestTerminalFocus(paneId)            consumeTerminalFocus(paneId) → true
                                             terminal.focus()
```

Deterministic: no timeouts, no polling. The `$effect` fires synchronously on `isReady` state change. `consumeTerminalFocus(paneId)` is idempotent (read-and-clear). Use this same pattern for future cross-view handoffs. The `paneId` is passed to Terminal via props (from PaneConversation or PaneTerminal) and accessed in QuestionCard/PlanCard via Svelte `getContext('paneId')`.

## Terminal Lock

Multi-user input gating. Solo users bypass it entirely.

| Scenario | Behavior |
|----------|----------|
| Solo user | Implicit control, lock not involved |
| Lock unclaimed | First keystroke auto-acquires |
| I hold lock | Input passes, green "Release" banner |
| Other holds lock | Input dropped, red "Take Control" banner |

Server is source of truth — client mirrors `TerminalLockUpdate` messages. `requestTerminalLock()` and `releaseTerminalLock()` are requests, not commands.

## Dimension Negotiation

Server sets PTY size to `min(all active viewports)`. Protocol:

| Message | When |
|---------|------|
| `TerminalVisible { rows, cols }` | Terminal becomes ready (server responds with `OutputHistory`) |
| `TerminalHidden` | Terminal unmounts |
| `Resize { rows, cols }` | Container resized (ResizeObserver, only when visible) |

Uses captured `mountedInstanceId` (not the store) to avoid races during instance switches.
