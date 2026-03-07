---
version: "1.3"
lastUpdated: "2026-03-07"
lifecycle: "active"
stakeholder: "all"
changeTrigger: "Session activity"
dependencies: ["projectbrief.md"]
---

# activeContext

## Current Status

**Phase**: Deployed to production

**Recent Activity**:

- 2026-03-07: Production deployment and cleanup
  - Deployed to .14 (x86_64) and .16 (aarch64) with new authority system
  - Configured keeper identity (pknull) on both machines
  - Removed legacy allowlist code (author_allowlist, user_allowlist)
  - Single source of truth: `~/.servitor/authority.toml`
  - Created GitHub repo: https://github.com/pknull/servitor (private)
  - 69 tests passing (61 unit + 8 integration)
- 2026-03-06: Person/Place/Skill authorization implemented
  - New `authority` module with Keeper, Permission, pattern matching
  - Authorization integrated into hook mode, daemon mode (SSE + Discord)
  - Skill permission checks in AgentExecutor before tool execution
- 2026-03-05: v1 clean-room rebuild complete
  - All core modules reimplemented with tighter code
  - New features: context fetching, cron scheduler, SSE subscription

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

## Deployment

| Host | Arch | LLM Provider | Status |
|------|------|--------------|--------|
| 172.16.0.14 (homebox2) | x86_64 | claude-code | Running |
| 172.16.0.16 (openclaw) | aarch64 | codex | Running |

Both configured with keeper "pknull" having full access.

## Deferred (v2)

- Consumer groups
- Capability challenges
- Reputation tracking

## Known Issues

None currently.

## Next Steps

- Add MCP servers to deployed Servitors (shell, docker, etc.)
- Test end-to-end task flow via Discord
- Consider adding more keepers for automation use cases

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
