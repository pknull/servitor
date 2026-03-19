---
panel_id: "2026-03-18--servitor-daemon-refactor"
phase: 1
phase_name: "Framing"
started: "2026-03-18T19:35:00+10:00"
completed: "2026-03-18T19:36:00+10:00"
---

# Phase 1: Framing

## Topic

Design the extraction of servitor's daemon loop from main.rs (1,655 lines) into maintainable modules with clear boundaries.

## Inferred Goals

1. Reduce main.rs from 1,655 lines to <200 lines (CLI + orchestration only)
2. Extract daemon loop into dedicated module with clear boundaries
3. Improve testability of event handling logic
4. Maintain existing behavior (no functional changes)
5. Follow existing codebase patterns (match other src/ modules)

## Constraints

- All 129 existing tests must pass
- No circular dependencies
- Maintain backwards compatibility (CLI interface unchanged)
- Incremental migration (can be done in phases)

## Decision Rule

Consensus

## Panel Composition

### Core Roles

- **The Moderator** — Process management, synthesis
- **The Analyst** — Workforce recruitment, capability matching
- **The Challenger** — Opposition, proof of necessity

### Recruited Specialists

| Agent | Session Name | Score | Rationale |
|-------|--------------|-------|-----------|
| code:architect | The Systems Designer | 10 | Module boundary design expertise |
| refactoring-specialist | The Extraction Surgeon | 9 | Safe code extraction |
| code:code-reviewer | The Quality Auditor | 7 | Quality assessment |

## Recruitment Rationale

This is a code architecture task requiring:

1. Systems thinking for module boundaries (architect)
2. Safe extraction techniques (refactoring specialist)
3. Quality validation (code reviewer)

No security-critical decisions; standard consensus rule applies.
