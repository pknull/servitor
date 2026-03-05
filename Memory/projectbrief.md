---
version: "1.0"
lastUpdated: "2026-03-05"
lifecycle: "active"
stakeholder: "technical"
changeTrigger: "Scope changes, major pivots"
dependencies: []
---

# Servitor Project Brief

## Overview

Servitor is a Rust daemon that executes tasks from the egregore decentralized network using MCP servers as capabilities. It implements the **ZeroClaw pattern**: Servitor owns MCP clients directly, no proxy layer. An LLM provides reasoning, emitting `tool_use` blocks that Servitor executes against scoped MCP servers, publishing signed attestations back to egregore.

**Name etymology**: Occult term for a created thoughtform that performs specific tasks — "like software that does one thing well."

## Architecture

### Three-Plane Model

| Plane | Purpose | Examples |
|-------|---------|----------|
| **Communication** | Message transport | Egregore, Discord, TUI |
| **Tool** | Execution capabilities | MCP servers (Docker, Shell) |
| **LLM** | Inference/reasoning | Claude, Ollama, OpenAI |

### Key Components

- **MCP Client Pool**: Owns stdio/http MCP clients directly
- **Scope Enforcer**: Allow/block policies per MCP server
- **Agent Loop**: tool_use → execute → feed_back cycle
- **Identity**: Ed25519 signing for attestations
- **Egregore Integration**: Hook receiver + HTTP publish

## Constraints

- Rust (matches egregore architecture)
- All LLM providers compiled in, runtime selection
- Scope enforcement non-negotiable (block takes precedence)
- Signed attestations for all results

## Dependencies

- `egregore`: Network protocol, identity patterns
- MCP servers: Tool capabilities
- LLM providers: Anthropic, OpenAI, Ollama

## Status

Phase 1 complete: Foundation scaffolding, config, identity, egregore integration, MCP clients, scope enforcement, agent loop.
