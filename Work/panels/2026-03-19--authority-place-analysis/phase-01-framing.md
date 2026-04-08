---
panel_id: "2026-03-19--authority-place-analysis"
phase: 1
phase_name: "Framing"
started: "2026-03-19T19:00:00+10:00"
topic: "Is 'Place' as a part of the Person/Place/Skill authorization system useful, or over-engineering?"
decision_rule: "consensus"
---

# Phase 1: Framing

## Topic Statement

Servitor implements a **Person/Place/Skill** authorization model. This panel assesses whether the **Place** dimension provides meaningful security value or represents unnecessary complexity.

## Core Question

> Should the "Place" dimension remain in the authorization model, be simplified, or be removed entirely?

## Definitions

| Term | Definition |
|------|------------|
| **Person (Keeper)** | Identity authenticated via credentials across planes (egregore pubkey, Discord user ID, HTTP bearer token) |
| **Place** | Origin context of a request, expressed as hierarchical colon-delimited pattern (e.g., `discord:guild:channel`, `egregore:local`, `a2a:server`) |
| **Skill** | Capability being invoked, expressed as pattern (e.g., `shell:execute`, `docker:*`) |

## Current Implementation Summary

### Place Values in Codebase

| Place Pattern | Source | Usage |
|---------------|--------|-------|
| `discord:{guild}:{channel}` | `daemon_handlers.rs:58` | Discord message origin |
| `egregore:local` | `daemon_handlers.rs:179` | Egregore SSE/hook task origin |
| `a2a:server` | `a2a/server/handlers.rs:245` | A2A JSON-RPC request origin |
| `*` | Example configs | Wildcard (anywhere) |

### Authorization Flow

```
Request arrives -> Extract PersonId -> Extract Place -> Check permissions
                                                       (pattern_matches on place + skill)
```

## Panel Goals

1. **Determine if Place provides authorization value** beyond audit logging
2. **Identify concrete scenarios** where Place-based denial is meaningful
3. **Assess implementation cost** versus security benefit
4. **Reach consensus** on recommendation: keep, simplify, or remove

## Constraints

1. **No regression in security posture** - any change must maintain or improve security
2. **Maintain auditability** - Place information must remain in logs regardless of authorization role
3. **Backward compatibility** - existing `authority.toml` files should continue to work (with possible warnings)
4. **Simplicity preference** - if two options provide equivalent security, prefer the simpler one

## Decision Rule

**Consensus** - All specialists must agree on the recommendation. Dissent must be documented with clear rationale and may result in escalation to user decision.

## Key Questions for Specialists

### For The Architect (security-auditor)

1. In ABAC/PBAC literature, what is the theoretical role of "context" or "environment" attributes?
2. Does Place provide defense-in-depth, or is it redundant with Person?
3. What threat models does Place address that Person+Skill cannot?

### For The Operator (devops-engineer)

1. What is the configuration burden of maintaining Place patterns?
2. In practice, when would an operator want Place-based restrictions?
3. Does Place add cognitive load that outweighs its benefits?

### For The Scholar (research-assistant)

1. How do comparable systems (HashiCorp Boundary, Teleport, SPIFFE) handle origin-based access?
2. Is there academic precedent for multi-dimensional authorization including "where"?
3. What do security frameworks (NIST, ISO 27001) say about context-aware authorization?

### For The Challenger (analyst)

1. Steelman the strongest case FOR Place - what scenario absolutely requires it?
2. Steelman the strongest case AGAINST Place - why is it over-engineering?
3. What hidden assumptions exist in either position?

## Evidence to Consider

### Observations from Codebase

1. **Redundancy concern**: Discord identity (`PersonId::Discord`) is inherently tied to Discord origin. If Place is `discord:*`, is explicit Place checking redundant?

2. **A2A case**: `a2a:server` Place is used with HTTP bearer tokens. But the bearer token itself identifies the caller - why additionally check Place?

3. **Egregore case**: `egregore:local` distinguishes local node from remote. But egregore pubkeys already identify the source feed.

### From `authority.example.toml`

```toml
# Limited automation access -- only from local egregore, only docker inspect
[[permission]]
keeper = "automation"
place = "egregore:local"
skills = ["docker:inspect_*", "docker:logs"]
```

**Question**: If `automation` identity only exists in egregore context, does restricting to `egregore:local` add value? Could `automation` identity arrive via Discord?

## Potential Outcomes

| Option | Description | Impact |
|--------|-------------|--------|
| **Keep** | Place remains as authorization dimension | No code change, document rationale |
| **Simplify** | Place used only for audit, not authorization | Remove Place from `Permission.matches()`, keep in logs |
| **Remove** | Place eliminated entirely | Simplify config schema, reduce code paths |
| **Defer** | Insufficient evidence, gather more data | Identify what additional evidence needed |

## Success Criteria

Panel succeeds if it produces:

1. Clear recommendation (one of the outcomes above)
2. Documented rationale supporting the recommendation
3. Minority dissent recorded (if any)
4. Action items for implementation (if change recommended)
