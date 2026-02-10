# Claude Conversations Library

A Rust library for reading and analyzing Claude conversation logs from the local `.claude` directory, structured similarly to the `claude_settings` package.

## Overview

Claude stores conversation data in JSONL files within `~/.claude/projects/`. This library provides:
- Type-safe structures for conversations and messages
- Convenient APIs for reading conversation history
- Query capabilities for filtering and searching messages
- Support for reading global history and project-specific conversations

## Directory Structure

```
~/.claude/
├── projects/
│   ├── -Users-alice-project1/
│   │   ├── session-uuid-1.jsonl
│   │   └── session-uuid-2.jsonl
│   └── -Users-bob-project2/
│       └── session-uuid-3.jsonl
└── history.jsonl
```

## Usage

Add to your BUILD file:
```python
rust_library(
    name = "my_lib",
    deps = [
        "//packages/claude_convo",
    ],
)
```

### Basic Example

```rust
use claude_convo::{ClaudeConvo, ConversationQuery, MessageRole};

// Create a conversation manager
let manager = ClaudeConvo::new();

// List all projects with conversations
let projects = manager.list_projects()?;

// Read a specific conversation
let convo = manager.read_conversation(
    "/Users/alice/project",
    "session-uuid-123"
)?;

// Query messages
let query = ConversationQuery::new(&convo);
let user_messages = query.by_role(MessageRole::User);

// Search for text
let results = query.contains_text("implement feature");

// Read global history
let history = manager.read_history()?;
```

### Reading Conversations

```rust
use claude_convo::ClaudeConvo;

let manager = ClaudeConvo::new();

// List all conversations in a project
let sessions = manager.list_conversations("/path/to/project")?;

// Read conversation metadata (without loading full content)
let metadata = manager.list_conversation_metadata("/path/to/project")?;

for meta in metadata {
    println!("Session {} has {} messages", meta.session_id, meta.message_count);
}
```

### Querying Messages

```rust
use claude_convo::{ClaudeConvo, ConversationQuery, MessageRole};

let manager = ClaudeConvo::new();
let convo = manager.read_conversation("/project", "session-id")?;

let query = ConversationQuery::new(&convo);

// Filter by role
let assistant_msgs = query.by_role(MessageRole::Assistant);

// Search for text
let matches = query.contains_text("error");

// Find tool uses
let bash_uses = query.tool_uses_by_name("Bash");

// Find errors
let errors = query.errors();
```

### Custom Paths

```rust
use claude_convo::{ClaudeConvo, PathResolver};

// Use custom Claude directory location
let resolver = PathResolver::new()
    .with_claude_dir("/custom/path/.claude");

let manager = ClaudeConvo::with_resolver(resolver);
```

## Running Tests

```bash
bazel test //packages/claude_convo:claude_convo_test --test_output=all
```

## Example Programs

Run the example to list conversations:
```bash
bazel run //packages/claude_convo/examples:list_conversations
```

## Architecture

The library follows a similar structure to `claude_settings`:

- **`lib.rs`** - Main library interface with `ClaudeConvo` manager
- **`error.rs`** - Error handling types
- **`paths.rs`** - Path resolution for `.claude` directories
- **`io.rs`** - File I/O operations for conversations
- **`types.rs`** - Data structures for conversations and messages
- **`reader.rs`** - JSONL parsing and conversation reading
- **`query.rs`** - Querying and filtering conversations

## Data Types

### Core Types
- `Conversation` - Collection of messages for a session
- `ConversationEntry` - Individual entry in a conversation (user/assistant messages, tool uses, etc.)
- `Message` - User or assistant message with content
- `ContentPart` - Parts of a message (text, tool use, tool result)
- `HistoryEntry` - Entry from the global history file

### Query Helpers
- `ConversationQuery` - Query builder for filtering conversation entries
- `HistoryQuery` - Query builder for filtering history entries