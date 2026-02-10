# Claude CLI - Conversation Explorer

A powerful command-line tool for exploring and analyzing Claude conversation data stored in your `~/.claude` directory. Built with Rust and Clap for maximum performance and ergonomics.

## Installation

```bash
bazel build //packages/claude_convo:claude-cli
```

## Usage

```bash
bazel run //packages/claude_convo:claude-cli -- [COMMAND] [OPTIONS]
```

Or build and use directly:
```bash
bazel build //packages/claude_convo:claude-cli
./bazel-bin/packages/claude_convo/claude-cli [COMMAND]
```

## Commands

### Overview
Get a quick summary of your Claude data:
```bash
claude-cli overview
```

### List Commands

List all projects with conversations:
```bash
claude-cli list projects
claude-cli list projects --with-counts  # Show conversation counts
```

List conversations:
```bash
claude-cli list convos                    # List ALL conversations from ~/.claude
claude-cli list convos --detailed         # With full metadata
claude-cli list convos "/Users/alex/project"  # From specific project only
```

Show recent conversations across all projects:
```bash
claude-cli list recent --count 20
```

### Show Commands

Display conversation content with various filters:
```bash
# Show latest conversation in a project
claude-cli show "/Users/alex/project" latest

# Show specific conversation (use session ID)
claude-cli show "/Users/alex/project" "session-uuid"

# Show first/last N messages
claude-cli show "/Users/alex/project" latest --head 10
claude-cli show "/Users/alex/project" latest --tail      # defaults to 10
claude-cli show "/Users/alex/project" latest --tail 5    # explicit count

# Follow mode - watch for new messages in real-time (like tail -f)
claude-cli show "/Users/alex/project" latest --tail -F         # tail 10 and follow
claude-cli show "/Users/alex/project" latest --tail 20 -F      # tail 20 and follow
claude-cli show "/Users/alex/project" latest -F                # defaults to tail 10 and follow

# Show message range (0-indexed)
claude-cli show "/Users/alex/project" latest --range-start 10 --range-end 20

# Filter by role
claude-cli show "/Users/alex/project" latest --role user
claude-cli show "/Users/alex/project" latest --role assistant

# Show raw JSON
claude-cli show "/Users/alex/project" latest --raw
```

### Search Commands

Search for text across conversations:
```bash
# Search all projects
claude-cli search "implement feature"

# Search specific project
claude-cli search "bug fix" --project "/Users/alex/project"

# Show context around matches
claude-cli search "error" --context 2

# Search for tool usage
claude-cli search "Bash" --tool

# Find conversations with errors
claude-cli search "" --errors
```

### Statistics

Get detailed statistics:
```bash
# Global stats across all projects
claude-cli stats --all

# Project statistics
claude-cli stats "/Users/alex/project"

# Specific conversation stats
claude-cli stats "/Users/alex/project" "session-id"
```

### History

Query the global history file:
```bash
# Show recent history
claude-cli history --last 20

# Search history
claude-cli history --grep "feature"

# Filter by project
claude-cli history --project "/Users/alex/project"
```

### Export

Export conversation data:
```bash
# Export as JSON
claude-cli export "/Users/alex/project" "session-id" --format json

# Export as Markdown
claude-cli export "/Users/alex/project" "session-id" --format markdown

# Include metadata
claude-cli export "/Users/alex/project" "session-id" --with-metadata
```

## Output Formats

The CLI supports multiple output formats via the global `--format` flag:
- `text` (default) - Human-readable text output
- `json` - Machine-parseable JSON
- `markdown` - Formatted Markdown

```bash
claude-cli list projects --format json
claude-cli show "/path" latest --format markdown
```

## Quiet Mode

Use `--quiet` to suppress informational output (useful for scripting):
```bash
claude-cli search "pattern" --quiet
```

## Examples

### Find and review recent work
```bash
# See what you've been working on
claude-cli list recent --count 5

# Review a specific project's conversations
claude-cli list convos "/Users/alex/my-project" --detailed

# Look at the latest conversation
claude-cli show "/Users/alex/my-project" latest --tail 20
```

### Analyze tool usage patterns
```bash
# Find all Bash command usage
claude-cli search "Bash" --tool

# Get statistics on a heavy session
claude-cli stats "/Users/alex/project" "session-id"
```

### Export for documentation
```bash
# Export a conversation as Markdown for documentation
claude-cli export "/Users/alex/project" "session-id" \
  --format markdown --with-metadata > conversation.md
```

### Search for specific topics
```bash
# Find all conversations about testing
claude-cli search "test" --context 1

# Find conversations with errors to debug
claude-cli search "" --errors
```

## Tips

1. **Session IDs**: You can use just the first 8 characters of a session ID in most commands
2. **Latest**: Use "latest" as the session ID to refer to the most recent conversation
3. **Pipe-friendly**: The CLI works well with Unix pipes and standard tools
4. **JSON output**: Use `--format json` for integration with other tools
