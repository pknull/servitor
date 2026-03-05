---
version: "1.1"
lastUpdated: "2026-03-05"
lifecycle: "active"
stakeholder: "all"
changeTrigger: "Session activity"
dependencies: ["projectbrief.md"]
---

# activeContext

## Current Status

**Phase**: v1 rebuild complete

**Recent Activity**:

- 2026-03-05: v1 clean-room rebuild complete
  - All core modules reimplemented with tighter code
  - 48 tests passing (40 unit + 8 integration)
  - New features: context fetching, cron scheduler, SSE subscription
  - CLI working: `servitor init`, `servitor info`, `servitor exec`, `servitor run`

## Implementation Status

| Module | Status | Notes |
|--------|--------|-------|
| config | Complete | TOML loading, validation, cron expression parsing |
| identity | Complete | Ed25519 generation, signing, SSB wire format |
| egregore | Complete | Hook, publish, context fetching, message schemas |
| mcp | Complete | McpClient trait, stdio + http transports, pool |
| scope | Complete | Glob matching, allow/block policies |
| agent | Complete | Provider abstraction, context history, tool_use loop |
| events | Complete | EventRouter, CronSource, SseSource, McpNotificationSource |

## New in v1

- **Context fetching**: `egregore/context.rs` queries feed for conversation history via `parent_id`
- **Cron scheduling**: `[[schedule]]` config triggers tasks on cron expressions
- **SSE subscription**: `egregore.subscribe = true` enables real-time task feed
- **MCP notifications**: `on_notification` config converts server events to tasks
- **Event router**: Unified dispatch from multiple sources (cron, SSE, MCP, hook)

## Deferred (v2)

- Consumer groups
- Capability challenges
- Reputation tracking

## Known Issues

None currently.

## Next Steps

- Integration test with real MCP server (mcp-server-shell)
- Test SSE subscription with running egregore daemon
- Test cron task execution in daemon mode

## Quick Reference

```bash
# Build
cargo build --release

# Test
cargo test

# Initialize identity
./target/release/servitor init

# Show config
./target/release/servitor -c servitor.example.toml info

# Execute task (requires ANTHROPIC_API_KEY)
./target/release/servitor exec "List files in ~/Documents"

# Daemon with cron + SSE
./target/release/servitor run

# Hook mode (stdin)
./target/release/servitor run --hook
```
