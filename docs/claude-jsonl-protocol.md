# Claude Code JSONL Conversation Protocol

A practical guide to how Claude Code stores and updates conversation state on
disk, written from observations of the real protocol and from implementing the
`toolpath-claude` / `toolpath-convo` library consumers in this codebase.

## Table of Contents

- [File Layout on Disk](#file-layout-on-disk)
- [JSONL Entry Format](#jsonl-entry-format)
- [Entry Types Reference](#entry-types-reference)
- [Conversation Lifecycle](#conversation-lifecycle)
- [Tool Use Flow](#tool-use-flow)
- [Interactive Tool Flows](#interactive-tool-flows)
- [Session Rotation](#session-rotation)
- [Important Edge Cases and Gotchas](#important-edge-cases-and-gotchas)
- [Session Discovery](#session-discovery)
- [The MergingWatcher Model](#the-mergingwatcher-model)
- [State Inference from JSONL](#state-inference-from-jsonl)

---

## File Layout on Disk

Claude Code stores its data under `~/.claude/`. The key paths are:

```
~/.claude/
├── projects/
│   └── <sanitized-project-path>/     # e.g., "-Users-alex-myproject"
│       ├── <session-id-1>.jsonl      # conversation log for session 1
│       ├── <session-id-2>.jsonl      # conversation log for session 2
│       └── ...
├── settings.json                     # user preferences
└── credentials.json                  # auth tokens
```

### Project Path Encoding

The project directory name is a sanitized version of the absolute working
directory path. Path separators are replaced with hyphens. For example, a
project at `/Users/alex/myproject` becomes the directory name
`-Users-alex-myproject`.

### Session IDs

Each conversation session gets a unique ID (UUID-like string) that becomes the
JSONL filename. A single project directory may contain many session files -- one
for each `claude` invocation or session rotation (context overflow, plan-mode
    transitions, etc.).

### Timing of File Creation

The JSONL file for a session is created when Claude Code starts. The file may
contain system/init entries before any user input is sent. This means a session
file can exist even before the user has typed anything.

---

## JSONL Entry Format

Each line in a session `.jsonl` file is a self-contained JSON object. Every entry has a common envelope:

```json
{
  "uuid": "unique-entry-id",
  "type": "user|assistant|system|progress|agent_progress|...",
  "timestamp": "2024-06-01T12:00:00.000Z",
  "message": { ... },       // OPTIONAL: present for conversation entries
  "subtype": "...",          // OPTIONAL: present for system metadata
  ...                        // additional fields vary by type
}
```

### Common Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `uuid` | string | Yes | Unique identifier for this entry |
| `type` | string | Yes | Entry type: `user`, `assistant`, `system`, `progress`, `agent_progress` |
| `timestamp` | string | Yes | ISO 8601 timestamp |
| `message` | object | No | Present for conversation entries (user/assistant). Absent for system metadata. |
| `subtype` | string | No | Sub-categorization for system entries (e.g., `turn_duration`, `init`) |

### The `message` Object

When `message` is present, it follows the Anthropic Messages API structure:

```json
{
  "role": "user|assistant",
  "content": "string or array of content parts",
  "stop_reason": null,
  "stop_sequence": null,
  "model": "claude-sonnet-4-20250514"
}
```

**Content can be a string or an array of content parts:**

String form (simple text):
```json
{"role": "user", "content": "Fix the login bug"}
```

Array form (mixed content -- text, tool use, tool results):
```json
{
  "role": "assistant",
  "content": [
    {"type": "text", "text": "Let me read that file."},
    {"type": "tool_use", "id": "t1", "name": "Read", "input": {"path": "src/auth.rs"}}
  ]
}
```

---

## Entry Types Reference

### `user` -- Human Input

Written when the user's message (or a tool result) is sent to the API.

**User-authored message:**
```json
{
  "uuid": "u1",
  "type": "user",
  "timestamp": "2024-06-01T12:00:00Z",
  "message": {
    "role": "user",
    "content": "Fix the authentication bug in login.rs"
  }
}
```

**Tool-result-only message** (no human text, just tool outputs):
```json
{
  "uuid": "u2",
  "type": "user",
  "timestamp": "2024-06-01T12:00:02Z",
  "message": {
    "role": "user",
    "content": [
      {
        "type": "tool_result",
        "tool_use_id": "t1",
        "content": "fn main() { ... }",
        "is_error": false
      }
    ]
  }
}
```

Tool-result-only user entries are a critical concept: they carry the outputs of
tool executions but contain no human-authored text. The `role` is `"user"`
because the Messages API requires tool results to come from the user role, but
these are machine-generated.

### `assistant` -- Claude's Response

Written when an API response completes. Each entry represents the output of one API call.

**Text-only response:**
```json
{
  "uuid": "a1",
  "type": "assistant",
  "timestamp": "2024-06-01T12:00:01Z",
  "message": {
    "role": "assistant",
    "content": "I'll fix that bug for you. Let me read the file first.",
    "stop_reason": null,
    "stop_sequence": null,
    "model": "claude-sonnet-4-20250514"
  }
}
```

**Response with tool use:**
```json
{
  "uuid": "a2",
  "type": "assistant",
  "timestamp": "2024-06-01T12:00:01Z",
  "message": {
    "role": "assistant",
    "content": [
      {"type": "text", "text": "Let me read the file."},
      {
        "type": "tool_use",
        "id": "t1",
        "name": "Read",
        "input": {"file_path": "src/auth.rs"}
      }
    ],
    "stop_reason": null,
    "stop_sequence": null
  }
}
```

### `system` -- Metadata Entries

System entries carry metadata about the conversation lifecycle. They have **no
`message` field**. Instead, they use `subtype` and top-level fields.

**`init` -- Session Initialization:**
```json
{
  "uuid": "i1",
  "type": "system",
  "subtype": "init",
  "timestamp": "2024-06-01T12:00:00Z"
}
```

**`turn_duration` -- Turn Completion Signal:**
```json
{
  "uuid": "td1",
  "type": "system",
  "subtype": "turn_duration",
  "timestamp": "2024-06-01T12:00:05Z",
  "durationMs": 3456,
  "costUSD": 0.05
}
```

This is the **most important entry for state inference** --
see [Important Edge Cases](#important-edge-cases-and-gotchas).

### `progress` -- Hook/Tool Progress

Progress entries track intermediate states during long operations:

```json
{
  "uuid": "p1",
  "type": "progress",
  "timestamp": "2024-06-01T12:00:01Z"
}
```

### `agent_progress` -- Sub-Agent Progress

When Claude delegates work via the Task tool, sub-agent activity is logged as `agent_progress` entries:

```json
{
  "uuid": "ap1",
  "type": "agent_progress",
  "timestamp": "2024-06-01T12:00:01Z",
  "agentId": "agent-abc-123",
  "data": {
    "type": "agent_progress",
    "agentId": "agent-abc-123",
    "message": {
      "role": "assistant",
      "content": [
        {"type": "text", "text": "Reading the file now."},
        {"type": "tool_use", "id": "t5", "name": "Read", "input": {"path": "foo.rs"}}
      ]
    }
  }
}
```

The nesting is notable: the outer object has `agentId` and `data`, and the
actual sub-agent message lives at `data.message` (sometimes double-nested as
`data.message.message`).

---

## Conversation Lifecycle

A typical conversation turn follows this sequence in the JSONL:

### Simple Text Turn (No Tools)

```
1. user       → User sends a message
2. assistant  → Claude responds with text
3. system     → turn_duration (end of turn)
```

### Agentic Turn (With Tools)

```
1. user       → User sends initial prompt
2. assistant  → Claude responds with text + tool_use
3. user       → Tool result(s) (tool_result_only, no human text)
4. assistant  → Claude continues with more text/tools
5. user       → More tool results
6. assistant  → Final response (text only, no tool_use)
7. system     → turn_duration (end of turn)
```

### Key Observations

- **Multiple assistant entries per agentic turn**: Each API call produces a
  separate `assistant` entry. A turn that uses 5 tools will have multiple
  assistant + user entry pairs.
- **Tool results arrive as `user` entries**: The Messages API requires tool
  results under the `user` role. These user entries contain only `tool_result`
  content parts and no human-authored text.
- **`turn_duration` marks the true end of a turn**: This system entry appears
  only when Claude is genuinely done with the entire turn -- after all tool
  calls, all API roundtrips, and any follow-up reasoning.

---

## Tool Use Flow

When Claude invokes a tool, the JSONL shows the following pattern:

### Non-Interactive Tool (e.g., Read, Bash, Grep)

```
assistant  → content includes {"type": "tool_use", "id": "t1", "name": "Read", ...}
user       → content includes {"type": "tool_result", "tool_use_id": "t1", ...}
```

The tool_result arrives almost immediately since these tools execute without
user interaction.

### Interactive Tool (e.g., Edit with permission prompt)

For tools requiring user approval, the flow has a gap:

```
assistant  → content includes {"type": "tool_use", "id": "t1", "name": "Edit", ...}
             (Claude Code shows permission prompt in terminal)
             ... user reviews and approves ...
user       → content includes {"type": "tool_result", "tool_use_id": "t1", ...}
```

**Permission prompts leave NO JSONL trace.** The time between the `assistant`
entry (containing the `tool_use`) and the `user` entry (containing the
`tool_result`) may be arbitrarily long. There is no JSONL entry for the
permission request itself or the user's approval/denial.

---

## Interactive Tool Flows

### AskUserQuestion

The `AskUserQuestion` tool is used when Claude needs to ask the user a question
and wait for a response. It has a distinctive flow involving `progress` entries:

```
assistant     → tool_use: AskUserQuestion(...)
progress      → subtype: PreToolUse (Claude Code about to present the question)
                ... user sees question, types answer ...
                ... GAP -- no JSONL entries while waiting for user ...
progress      → subtype: PostToolUse (user answered, Claude Code processing)
user          → tool_result with the user's answer
```

The `PreToolUse` and `PostToolUse` progress entries bracket the human
interaction period. The gap between them is the time the user spent reading and
answering.

---

## Session Rotation

Claude Code rotates to a new session file when:
- **Context window overflow**: the conversation exceeds the context limit
- **Plan-mode transitions**: switching between plan mode and implementation mode
- **Manual restart**: user restarts the session

When rotation occurs, the new session file is created and the old session's
metadata gets a `successor` pointer. The `toolpath-claude` library's
`ConversationMeta` tracks this:

```rust
pub struct ConversationMeta {
    pub id: String,
    pub started_at: Option<DateTime<Utc>>,
    pub last_activity: Option<DateTime<Utc>>,
    pub message_count: usize,
    pub file_path: Option<PathBuf>,
    pub predecessor: Option<String>,  // previous session ID
    pub successor: Option<String>,    // next session ID
}
```

The `MergingWatcher` detects rotations and emits them as pending rotation
events. The conversation watcher picks these up and broadcasts `SessionRotated`
events to connected clients.

---

## Important Edge Cases and Gotchas

### 1. `stop_reason` is ALWAYS `null` in Real JSONL

The `message.stop_reason` field in assistant entries is **always `null`** in
practice. Claude Code writes the entry while the streaming API response is in
progress; by the time the entry is persisted, the stop_reason from the API's
final event is not retroactively filled in.

**Implication**: You cannot rely on `stop_reason` to determine whether Claude
finished responding vs. was interrupted. Instead, the codebase infers `end_turn`
for every assistant entry:

```rust
// From conversation_watcher.rs:
let stop_reason = turn.stop_reason.clone().or_else(|| {
    if matches!(turn.role, Role::Assistant) {
        Some("end_turn".to_string())
    } else {
        None
    }
});
```

### 2. `turn_duration` is the ONLY Reliable End-of-Turn Signal

Since `stop_reason` is null, the `system` entry with `subtype: "turn_duration"`
is the **sole authoritative signal** that a complete turn has ended. It appears
after:
- All tool calls in the agentic loop have completed
- All API roundtrips are done
- Claude has produced its final response

The state manager treats `turn_duration` as "definitive idle" -- once received,
terminal heuristics (tool pattern matching) cannot override the WaitingForInput
state. This prevents false positives from tool names appearing in Claude's text
output (e.g., "I used Read(file) to check the contents").

### 3. Assistant Entries are Written Mid-Stream

During an agentic turn, multiple `assistant` entries appear -- one per API call.
Each assistant entry means "this API call completed," but does NOT mean the
overall turn is finished. Claude may continue with more tool calls.

The state manager treats each assistant entry as "tentative idle"
(WaitingForInput). Terminal heuristics CAN override tentative idle, which allows
non-interactive tools (Read, Bash) to show as `ToolExecuting` between the
assistant entry and the tool's completion.

### 4. Tool-Result-Only User Entries are Not Standalone Turns

User entries where `content` is an array containing only `tool_result` parts (no
text) are **not human input**. They are machine-generated wrappers required by
the Messages API. The `MergingWatcher` detects these and merges them into the
preceding assistant turn rather than emitting them as separate turns.

Detection logic:
```rust
fn is_tool_result_only(entry: &ConversationEntry) -> bool {
    let Some(msg) = &entry.message else { return false };
    msg.role == MessageRole::User
        && msg.text().is_empty()
        && !msg.tool_results().is_empty()
}
```

### 5. Permission Prompts Leave No JSONL Trace

When Claude invokes a tool that requires user permission (e.g., file writes,
shell commands with side effects), Claude Code displays a prompt in the
terminal. The user's acceptance or rejection of this permission is **not
recorded in the JSONL**. The only evidence is:
- An `assistant` entry with a `tool_use` in its content
- A gap in timestamps
- Eventually, a `user` entry with the `tool_result` (if approved) or no further
  entry for that tool_use_id (if denied/timed out)

### 6. The `type` Field Name Collision

The top-level `type` field in JSONL entries (e.g., `"type": "system"`) conflicts
with the `type` field inside content arrays (e.g., `"type": "tool_use"`). When
the `toolpath-claude` library parses entries, the top-level `type` is consumed
as `entry_type`. Downstream code must look for `subtype` in the extra fields,
not `type`.

This is why progress events emit `subtype` in their data bag:
```rust
// In MergingWatcher -- "type" was consumed, "subtype" preserved in extra:
WatcherEvent::Progress {
    kind: entry.entry_type.clone(),  // "system" (from "type")
    data: { "subtype": "turn_duration", ... }  // "subtype" preserved
}
```

### 7. Role Name Mismatch: "human" vs "user"

Claude Code JSONL uses `"role": "user"` in the message object, but some older or
internal representations used `"human"`. The `toolpath-convo` library normalizes
this to the `Role::User` enum variant. The state manager expects `"user"` as the
entry_type string. The conversation watcher performs this mapping:

```rust
let entry_type = match &turn.role {
    Role::User => "user".to_string(),
    Role::Assistant => "assistant".to_string(),
    Role::System => "system".to_string(),
    Role::Other(s) => s.clone(),
};
```

---

## Session Discovery

When `toolpath` manages Claude instances, it needs to discover which JSONL
session file belongs to which instance. This is non-trivial because multiple
instances can share the same working directory.

### Discovery Mechanism

1. **Wait for first input**: Discovery only begins after the instance has
   received its first user input. Without input, Claude cannot have created a
   session, so any candidates would belong to other instances.

2. **List candidate sessions**: Query `~/.claude/projects/<sanitized-path>/` for
   session files created after the instance's `created_at` timestamp.

3. **Filter claimed sessions**: Exclude sessions already claimed by other
   instances (tracked in-memory by the `GlobalStateManager`).

4. **Content verification**: For single candidates, verify the session contains
   text matching what was actually sent to this instance (prevents
   cross-instance theft).

5. **Ambiguous candidates**: If multiple unclaimed candidates remain, the system
   sends a `SessionAmbiguous` message to the client for manual selection.

### Race Condition Prevention

The `first_input_at` gate prevents a critical race condition: without it,
Instance A (created earlier, no input yet) would discover and claim Instance B's
session, because the session's `started_at` would be after Instance A's
`created_at`.

---

## The MergingWatcher Model

The `MergingWatcher` (in `ws/merging_watcher.rs`) wraps the upstream
`toolpath-claude` `ConversationWatcher` to handle a critical limitation: **tool
results arriving in a different poll cycle than their corresponding tool_use**.

### The Problem

The upstream watcher's `poll()` method reads new JSONL entries since the last
poll. It merges tool_result entries into their corresponding assistant turns --
but only within a single poll batch. If the tool_use was emitted in poll N and
the tool_result arrives in poll N+1, the result is silently dropped.

### The Solution

`MergingWatcher` maintains:
- `emitted_turns`: All turns emitted so far, keyed by turn ID
- `pending_tool_uses`: Maps `tool_use_id` to `turn_id` for unresolved tool uses

On each poll:

1. **Get raw entries** from the inner watcher
2. **Entries without `message`** become `Progress` events (system metadata,
   agent_progress, etc.)
3. **Tool-result-only user entries** are merged:
   - First, try same-batch merge (into events from this poll)
   - Then, try cross-poll merge (into previously emitted turns via
     `pending_tool_uses`)
4. **Regular entries** are converted to `Turn` events, with any tool_uses
registered in `pending_tool_uses`
5. **Cross-poll merges** produce `TurnUpdated` events (so the UI can replace the
stale turn)

### Event Types

The watcher emits three types of events:

| Event | Meaning |
|-------|---------|
| `Turn(turn)` | A new conversation turn (user message or assistant response) |
| `TurnUpdated(turn)` | An existing turn was updated (cross-poll tool result merged) |
| `Progress { kind, data }` | A non-message entry (system metadata, agent progress, etc.) |

---

## State Inference from JSONL

The codebase uses JSONL entries as the **authoritative** source for Claude's
state, supplemented by terminal output heuristics as a faster but less reliable
signal.

### Signal Priority

1. **`turn_duration`** (system entry): Definitive turn completion. Sets
   "definitive idle" -- cannot be overridden by terminal heuristics.
2. **`assistant` entry**: Tentative idle (one API call completed). CAN be
   overridden by terminal tool patterns for non-interactive tools.
3. **`user` entry**: Transition to Thinking state.
4. **Terminal output patterns** (e.g., `Read(`, `Bash(`): Heuristic tool
   detection. Only used to enrich state between authoritative JSONL signals.

### State Machine

```
                         user entry
Idle ──────────────────────────────────────→ Thinking
  ↑                                            │
  │                                            │ terminal output
  │  turn_duration                             ↓
  ├─────────────────────── WaitingForInput ← Responding
  │  (definitive)               ↑              │
  │                             │              │ tool pattern
  │  assistant entry            │              ↓
  └────────────────────→ WaitingForInput   ToolExecuting
                         (tentative)           │
                              ↑                │
                              └────────────────┘
                              (tool pattern overrides tentative only)
```

### Definitive vs. Tentative Idle

This distinction is critical for avoiding false positives:

- **Definitive idle** (from `turn_duration`): The turn is truly over. If the
  terminal output contains text like "I used Read(file) to check", the tool
  pattern match is ignored. This prevents the state from incorrectly showing
  `ToolExecuting` when Claude is actually done.

- **Tentative idle** (from `assistant` entry): An API call completed, but the
  agentic loop may continue. If the terminal then shows `Read(src/main.rs)` with
  a spinner, the state correctly transitions to `ToolExecuting`. This gives
  users faster feedback during multi-tool turns.

### Polling Interval

The conversation watcher polls the JSONL file every **500ms**
(`tokio::time::interval(Duration::from_millis(500))`). This means state
transitions from JSONL signals have up to 500ms latency. Terminal heuristics
provide sub-100ms feedback to bridge this gap.
