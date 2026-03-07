---
version: "1.2"
lastUpdated: "2026-03-06"
lifecycle: "active"
stakeholder: "all"
changeTrigger: "Session activity"
dependencies: ["projectbrief.md"]
---

# activeContext

## Current Status

**Phase**: Person/Place/Skill authorization complete

**Recent Activity**:

- 2026-03-06: Person/Place/Skill authorization implemented
  - New `authority` module with Keeper, Permission, pattern matching
  - Authorization integrated into hook mode, daemon mode (SSE + Discord)
  - Skill permission checks in AgentExecutor before tool execution
  - 70 tests passing (62 unit + 8 integration)
  - Example config: `authority.example.toml`
- 2026-03-05: v1 clean-room rebuild complete
  - All core modules reimplemented with tighter code
  - New features: context fetching, cron scheduler, SSE subscription
  - CLI working: `servitor init`, `servitor info`, `servitor exec`, `servitor run`

## Implementation Status

| Module | Status | Notes |
|--------|--------|-------|
| authority | Complete | Person/Place/Skill auth, pattern matching |
| config | Complete | TOML loading, validation, cron expression parsing |
| identity | Complete | Ed25519 generation, signing, SSB wire format |
| egregore | Complete | Hook, publish, context fetching, message schemas |
| mcp | Complete | McpClient trait, stdio + http transports, pool |
| scope | Complete | Glob matching, allow/block policies |
| agent | Complete | Provider abstraction, context history, tool_use loop, skill auth |
| events | Complete | EventRouter, CronSource, SseSource, McpNotificationSource |
| comms | Complete | Discord transport with authorization |

## Authority System

Person/Place/Skill authorization via `~/.servitor/authority.toml`:

- **Person (Keeper)**: Identity across planes (egregore pubkey, discord user ID, http token)
- **Place**: Where request originates (e.g., `discord:guild:channel`, `egregore:local`)
- **Skill**: What capabilities can be invoked (e.g., `shell:execute`, `docker:*`)

No authority.toml = open mode (accept all). Useful for development.

## New in v1.2

- **Authority module**: `src/authority/` with keeper, permission, pattern matching
- **Task metadata**: `author` and `keeper` fields on Task for tracking
- **Skill enforcement**: AgentExecutor checks skill permissions before tool calls
- **Info command**: Shows authority status (open mode vs restricted)

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
- Test Discord with authority restrictions

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
