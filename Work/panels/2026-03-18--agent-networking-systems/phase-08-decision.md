---
panel_id: "2026-03-18--agent-networking-systems"
phase: 8
phase_name: "Decision"
started: "2026-03-18T16:45:00+10:00"
completed: "2026-03-18T17:00:00+10:00"
---

# Phase 8: Decision Report

## Topic

Find other agent networking systems. Decentralized alone is not a selling point — compare agent networking tools to our setup.

## Inferred Goals

1. Identify agent networking tools beyond "decentralization" framing
2. Compare coordination protocols and message passing mechanisms
3. Find practical differentiators for Servitor/Egregore
4. Determine what actually matters for agent-to-agent communication

## Decision Rule

Consensus

## Panel Composition

**Core Roles**:

- The Moderator (Facilitator)
- The Analyst (Workforce Intelligence)
- The Challenger (Opposition & Quality Gate)

**Recruited Specialists**:

| Agent | Session Name | Score | Role |
|-------|--------------|-------|------|
| research-assistant | The Protocol Archaeologist | 9 | Agent networking protocols |
| devops-engineer | The Integration Analyst | 7 | Deployment patterns |
| security-auditor | The Trust Cartographer | 6 | Trust and identity models |

---

## Key Findings

### Agent Networking Paradigms

| Paradigm | Systems | Servitor Fit |
|----------|---------|--------------|
| Centralized Orchestration | CrewAI, LangGraph, AutoGen, Swarm | ❌ Not this |
| Standardized Protocols | A2A, MCP, FIPA-ACL | ✅ Uses MCP |
| Actor/Event Models | Ray, Akka, Kafka | ⚠️ Similar patterns |
| Gossip Replication | Egregore, libp2p | ✅ This is us |

**Finding**: Gossip-based agent networking is RARE. Only Egregore and libp2p use it. Most frameworks use centralized coordinators.

### Trust Model Comparison

| Framework | Identity | Audit Trail | Verifiable? |
|-----------|----------|-------------|-------------|
| OpenAI Swarm | String | None | ❌ |
| AutoGen | AgentId tuple | Ephemeral | ❌ |
| LangChain | None | External (LangSmith) | ❌ Trust provider |
| CrewAI | JWS-signed cards | Mutable logs | ⚠️ Trust DB |
| **Servitor** | Ed25519 keypair | Append-only feed | ✅ Self-verifiable |

**Finding**: Most frameworks have NO trust model. CrewAI/A2A has authentication. Only Servitor has third-party verifiable proof.

### Latency Reality

| Pattern | Latency | Systems |
|---------|---------|---------|
| In-memory | <1ms | CrewAI, Swarm |
| gRPC | 1-10ms | AutoGen |
| **Gossip** | 100ms-10s | Egregore |

**Finding**: Gossip is 100-1000x slower than in-memory. Not suitable for real-time multi-turn chat.

---

## Decision

### Primary Strategy: Compliance Positioning with Edge Secondary

**Stop saying**: "Decentralized," "Gossip protocol," "Cryptographic signatures"

**Start saying**:

- "Provable AI decisions"
- "Agents that work offline"
- "Multi-org coordination without shared infrastructure"
- "Audit trails that can't be tampered with"

### Target Use Cases

| Use Case | Why We Win |
|----------|------------|
| Financial compliance | Third-party verifiable audit trail |
| Healthcare AI | Prove which agent recommended what |
| Edge/Offline | No cloud dependency |
| Multi-org collaboration | No shared infrastructure needed |
| Legal/Government | Tamper-proof decision records |

### Where NOT to Compete

| Use Case | Why We Lose | Who Wins |
|----------|-------------|----------|
| Real-time chat orchestration | Gossip latency | CrewAI, LangGraph |
| Simple single-org agents | Over-engineered | Swarm, AutoGen |
| Dashboard-driven workflows | No central UI | Dify, n8n |

---

## Consensus

**Consensus Level**: 85% (Strong)

| Panelist | Aligned | Notes |
|----------|---------|-------|
| The Protocol Archaeologist | ✅ Yes | Gossip is niche but valuable for target use cases |
| The Integration Analyst | ✅ Yes | Async-first positioning makes latency acceptable |
| The Trust Cartographer | ✅ Yes | Cryptographic proof is real differentiator |
| The Challenger | ⚠️ Partial | Accepts positioning but demands user validation |

### Dissent (15%)

**The Challenger**: "The positioning is better than 'decentralized,' but I still don't see users asking for this. Find one customer in financial services or healthcare who would pay for verifiable AI audit trails before building more features."

---

## Next Steps

| Priority | Action | Owner | Deliverable |
|----------|--------|-------|-------------|
| P0 | Stop using "decentralized" in messaging | Marketing | Updated README, docs |
| P1 | Create "Provable AI" landing page | Documentation | Single-page explainer |
| P1 | Find 1 compliance-sensitive user | Outreach | User interview |
| P2 | Document offline operation mode | Documentation | Edge deployment guide |
| P2 | A2A/MCP interop exploration | Engineering | Spike: can we bridge? |

---

## Appendix: Competitive Matrix

| Capability | CrewAI | LangGraph | AutoGen | Swarm | Servitor |
|------------|--------|-----------|---------|-------|----------|
| Multi-turn orchestration | ✅ | ✅ | ✅ | ✅ | ⚠️ Async |
| Real-time latency | ✅ | ✅ | ⚠️ | ✅ | ❌ |
| Offline operation | ❌ | ❌ | ❌ | ❌ | ✅ |
| Multi-org coordination | ⚠️ A2A | ❌ | ❌ | ❌ | ✅ |
| Verifiable audit trail | ❌ | ❌ | ❌ | ❌ | ✅ |
| No central coordinator | ❌ | ❌ | ❌ | ❌ | ✅ |
| MCP tooling | ✅ | ✅ | ⚠️ | ❌ | ✅ |
| Ease of setup | ✅ | ✅ | ⚠️ | ✅ | ⚠️ |
| GitHub stars | 33K | 15K | 40K | 20K | — |
