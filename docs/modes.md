# Servitor Modes

Servitor is an executor. Its runtime modes change where tasks come from and where results go, but they do not change the core role: execute pre-planned work, enforce authority and scope, publish results.

## Architecture Overview

```text
┌─────────────────────────────────────────────────────────────┐
│                        Servitor                              │
│                                                              │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐    │
│  │ Egregore │  │   A2A    │  │   MCP    │  │Authority │    │
│  │  Client  │  │Server/Cli│  │   Pool   │  │ + Scope  │    │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘    │
└─────────────────────────────────────────────────────────────┘
```

## Mode Summary

| Mode | Egregore | A2A Server | A2A Client | MCP | Use Case |
|------|----------|------------|------------|-----|----------|
| Daemon executor | ✓ | optional | optional | ✓ | Normal network-connected executor |
| Local direct exec | optional | ✗ | optional | ✓ | One-shot execution of structured tool calls |
| Worker | ✗ | ✓ | ✗ | ✓ | Capability endpoint for external agents |
| Coordinator | ✓ | ✓ | ✓ | optional | Routes work to external workers |
| Gateway | ✓ | ✓ | ✗ | ✗ | Minimal feed-to-A2A bridge |

## Mode 1: Daemon Executor

`servitor run`

Use when Servitor should:

- subscribe to Egregore tasks over SSE
- publish profile and heartbeat data
- execute local MCP-backed tasks
- optionally delegate to configured A2A agents

This is the standard networked mode.

## Mode 2: Local Direct Exec

`servitor exec '<json tool calls>'`

Use when you already know the exact tool calls and want Servitor to execute them locally under its normal authority and scope checks.

Example:

```bash
servitor exec '[{"name":"shell__execute","arguments":{"command":"date"}}]'
```

This mode does not do task planning.

## Mode 3: Worker

Use when Servitor should expose execution capabilities to external agents over A2A without joining the Egregore task flow.

Characteristics:

- no user-facing conversation
- no planning
- receives structured work and executes it

## Mode 4: Coordinator

Use when Servitor should subscribe to Egregore tasks, apply routing logic, and delegate execution to external A2A workers.

Characteristics:

- low local execution surface
- useful as a routing node
- still publishes feed-visible outcomes and profiles

## Mode 5: Gateway

Use when you need the smallest Servitor footprint that still bridges Egregore task sourcing to an A2A boundary.

Characteristics:

- little or no local MCP execution
- no planning
- primarily moves work between transports

## Configuration Guidance

- `[mcp.*]` is required for local execution capability
- `[a2a_server]` enables inbound A2A task handling
- `[a2a.*]` enables outbound delegation to external agents
- `[egregore] subscribe = true` enables SSE-fed network participation
- `[heartbeat]` controls profile publication cadence
- `[agent] publish_trace_spans = true` enables feed-visible execution traces

If you need planning, conversation, or user interaction, use Familiar. Servitor stays the executor.
