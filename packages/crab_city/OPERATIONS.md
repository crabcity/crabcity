# Crab City Server Operations Guide

## Overview

Crab City is a multi-instance Claude Code manager with a web UI. This document covers operational concerns for running and maintaining the server.

## Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `CRAB_CITY_MAX_BUFFER_MB` | Maximum output buffer per instance (MB) | 1 |
| `CRAB_CITY_MAX_HISTORY_KB` | Maximum history bytes sent on focus switch (KB) | 64 |
| `CRAB_CITY_HANG_TIMEOUT_SECS` | Hang detection timeout (0 = disabled) | 300 |
| `RUST_LOG` | Logging level/filter | `crab_city=info` |

### Command Line Arguments

```bash
crab_city [OPTIONS]

Options:
  -p, --port <PORT>              Port for web server (0 = auto-select) [default: 0]
  -b, --host <HOST>              Host to bind to [default: 127.0.0.1]
      --instance-base-port       Base port for instances [default: 9000]
      --default-command          Default command for new instances
  -d, --debug                    Enable debug logging
      --data-dir <PATH>          Custom data directory [default: ~/.crabcity]
      --reset-db                 Reset database (with confirmation)
      --import-all               Import all existing conversations
      --import-from <PATH>       Import from specific project directory
```

## Endpoints

### Health Check

```bash
curl http://localhost:PORT/health
```

Response:
```json
{
  "status": "healthy",
  "instances": {"total": 2, "active": 2},
  "connections": 1,
  "uptime_secs": 3600
}
```

Status values:
- `healthy` - No errors recorded
- `degraded` - Errors have occurred (check metrics)

### Metrics

```bash
curl http://localhost:PORT/metrics
```

Response:
```json
{
  "uptime_secs": 3600,
  "connections": {"active": 1, "total": 5},
  "instances": {"active": 2, "total_created": 3, "stopped": 1},
  "messages": {"received": 1000, "sent": 5000, "dropped": 0},
  "errors": {"pty": 0, "websocket": 0},
  "performance": {"focus_switches": 10, "history_replays": 10, "history_bytes_sent": 640000}
}
```

### Key Metrics to Monitor

| Metric | Healthy Range | Action if Exceeded |
|--------|---------------|-------------------|
| `messages.dropped` | 0 | Client too slow, check network |
| `errors.pty` | 0 | Check system PTY limits |
| `errors.websocket` | Low | Check client connectivity |
| `connections.active` | Expected users | Investigate if much higher |

## Error Scenarios and Recovery

### WebSocket Connection Drops

**Symptom:** Client disconnects unexpectedly

**Automatic Recovery:**
- Focus tasks cancelled via CancellationToken
- Connection metrics updated
- No manual intervention needed

**Manual Recovery:** Refresh browser

### Instance Process Exits

**Symptom:** Claude CLI process terminates unexpectedly

**Detection:**
- Instance marked as `running: false`
- Check `/metrics` for `instances.stopped` count

**Recovery:**
1. Review logs for exit reason
2. User can create new instance from UI
3. Check if Claude CLI itself has issues

### PTY Allocation Failure

**Symptom:** Instance creation fails with PTY error

**Cause:**
- System PTY limit reached (`/dev/pts` exhausted)
- File descriptor limit reached
- Permissions issue

**Recovery:**
1. Check file descriptor limit: `ulimit -n`
2. Check PTY availability: `ls /dev/pts | wc -l`
3. Increase limits in `/etc/security/limits.conf`
4. Restart server if needed

### High Memory Usage

**Symptom:** Server memory growing over time

**Cause:** Large output buffers from verbose Claude instances

**Investigation:**
1. Check `/metrics` for `performance.history_bytes_sent`
2. Count active instances: `instances.active`

**Recovery:**
1. Reduce `CRAB_CITY_MAX_BUFFER_MB` if needed
2. Stop idle instances
3. Restart server to clear buffers

### State Detection Incorrect

**Symptom:** UI shows wrong Claude state (e.g., stuck on "Thinking")

**Cause:**
- Terminal pattern mismatch
- Conversation JSONL not updating

**Workaround:** State will self-correct on next conversation entry

**Long-term:** State detection uses conversation JSONL as authoritative source - terminal patterns are supplementary

### Backpressure / Message Drops

**Symptom:** `messages.dropped` > 0 in metrics

**Cause:** Client WebSocket can't keep up with output rate

**Impact:** Some terminal output missed (not critical - data still in buffer)

**Notification:** Client receives `OutputLagged` message to show indicator

**Recovery:**
1. Check client network connection
2. Reduce verbose output from Claude if possible
3. Consider filtering output client-side

## Database Management

### Location

Default: `~/.crabcity/crabcity.db` (SQLite)

### Backup

```bash
# Manual backup
cp ~/.crabcity/crabcity.db ~/.crabcity/backup-$(date +%Y%m%d).db

# Using API
curl -X POST http://localhost:PORT/api/admin/backup
```

### Reset

```bash
# With confirmation prompt
crab_city --reset-db
```

### Import Conversations

```bash
# Import all Claude Code conversations
crab_city --import-all

# Import from specific project
crab_city --import-from /path/to/project

# Via API (while running)
curl -X POST http://localhost:PORT/api/admin/import \
  -H "Content-Type: application/json" \
  -d '{"import_all": true}'
```

## Graceful Shutdown

Server handles `SIGTERM`:

1. Stops accepting new connections
2. Cancels all focus tasks
3. Stops all instances (sends SIGTERM to Claude processes)
4. Closes database connections
5. Exits

**Recommended shutdown timeout:** 30 seconds

```bash
# Graceful stop
kill -TERM $(pgrep crab_city)

# Wait for clean shutdown
sleep 30

# Force if needed
kill -9 $(pgrep crab_city)
```

## Logging

### Log Levels

```bash
# Default
RUST_LOG=crab_city=info

# Debug mode
RUST_LOG=crab_city=debug

# Trace everything
RUST_LOG=crab_city=trace

# Specific modules
RUST_LOG=crab_city::multiplexed_ws=debug,crab_city::inference=trace
```

### Key Log Messages

| Message | Meaning |
|---------|---------|
| `New multiplexed WebSocket connection` | Client connected |
| `WebSocket connection closed` | Client disconnected |
| `State changed` | Claude state transition |
| `PTY output lagged by N messages` | Backpressure detected |
| `Instance actor stopped` | Instance cleanly shut down |

## Troubleshooting Checklist

1. **Server won't start**
   - Check port availability: `lsof -i :PORT`
   - Check data directory permissions
   - Review logs with `RUST_LOG=debug`

2. **Instances won't create**
   - Check `claude` command exists: `which claude`
   - Check PTY limits: `ulimit -n`
   - Review PTY error count in metrics

3. **UI shows stale state**
   - Check WebSocket connection in browser devtools
   - Look for `OutputLagged` messages
   - State will self-correct on next conversation update

4. **High latency**
   - Check `messages.dropped` metric
   - Review `history_bytes_sent` for large replays
   - Consider reducing max history size

5. **Database corruption**
   - Stop server
   - Backup current database
   - Use `--reset-db` to start fresh
   - Re-import conversations with `--import-all`

## Architecture Notes

### WebSocket Multiplexing

- Single WebSocket connection per client
- High-bandwidth: Terminal output from ONE focused instance
- Low-bandwidth: State changes from ALL instances
- Focus switch triggers bounded history replay (max 64KB by default)

### State Detection

Priority of state signals (highest to lowest):
1. Conversation JSONL `turn_duration` entry (authoritative)
2. Conversation JSONL `end_turn` stop_reason (authoritative)
3. Terminal output patterns (heuristic)
4. Timeout fallback (safety net, 10 seconds)

### Instance Management

- Each instance runs as a Tokio task (actor model)
- PTY managed by `pty_manager` crate
- Output buffered up to `CRAB_CITY_MAX_BUFFER_MB`
- Clean shutdown via `CancellationToken`
