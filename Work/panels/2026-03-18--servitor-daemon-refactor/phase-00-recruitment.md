---
panel_id: "2026-03-18--servitor-daemon-refactor"
phase: -1
phase_name: "Topic Analysis & Workforce Recruitment"
started: "2026-03-18T19:30:00+10:00"
completed: "2026-03-18T19:31:00+10:00"
---

# Phase -1: Topic Analysis & Workforce Recruitment

## Topic Analysis

**Domain**: Rust architecture, code organization, maintainability refactoring
**Key Concerns**: Module boundaries, code extraction, async runtime patterns, testability
**Scope**: servitor/src/main.rs (1,655 lines) containing CLI, daemon loop, event handling

### Current Structure (main.rs)

| Function | Lines | Purpose |
|----------|-------|---------|
| `run_daemon_mode` | 330-722 | Main event loop (~390 lines) |
| `process_sse_message` | 724-801 | SSE message processing |
| `maybe_accept_assignment` | 803-875 | Task assignment decision |
| `execute_assigned_task` | 876-993 | Task execution |
| `task_matches_capabilities` | 994-1004 | Capability matching |
| `task_from_comms` | 1005-1045 | Comms message conversion |
| `publish_auth_denied_event` | 1046-1078 | Auth denial publishing |
| `run_exec` | 1079-1179 | Direct execution mode |
| `run_info` | 1185-1256 | Info display |
| `run_init` | 1257-1282 | Initialization |
| `build_profile` | 1283-1325 | Profile construction |
| Helper functions | 1326-1420 | Config, authority, hashing |

## Required Expertise Areas

1. **Rust Architecture** — Module organization, visibility, crate structure
2. **Async Patterns** — tokio::select!, event loops, state management
3. **Refactoring** — Safe extraction, minimal disruption, test preservation
4. **Code Quality** — File size limits, cohesion, coupling analysis

## Agent Scoring

| Agent | Score | Rationale |
|-------|-------|-----------|
| code:architect | 10 | Perfect match for module boundary design |
| refactoring-specialist | 9 | Safe code extraction expertise |
| code:code-reviewer | 7 | Quality assessment, pattern validation |
| code:debugger | 5 | Can identify coupling issues |

## Recruited Specialists

1. **code:architect** → **"The Systems Designer"** (score: 10)
2. **refactoring-specialist** → **"The Extraction Surgeon"** (score: 9)
3. **code:code-reviewer** → **"The Quality Auditor"** (score: 7)

## Inferred Goals

1. Reduce main.rs from 1,655 lines to <400 lines (CLI + orchestration only)
2. Extract daemon loop into dedicated module with clear boundaries
3. Improve testability of event handling logic
4. Maintain existing behavior (no functional changes)
5. Follow existing codebase patterns (match other src/ modules)

## Decision Rule

Consensus (architectural decision, not security-critical)
