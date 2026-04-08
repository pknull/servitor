---
panel_id: "2026-03-19--authority-place-analysis"
phase: 0
phase_name: "Topic Analysis & Workforce Recruitment"
started: "2026-03-19T18:30:00+10:00"
topic: "Is 'Place' as a part of the security system useful, or meaningful?"
decision_rule: "consensus"
---

# Phase 0: Topic Analysis & Workforce Recruitment

## Topic Analysis

**Question**: Does the "Place" dimension in Servitor's Person/Place/Skill authorization model provide meaningful security value, or is it over-engineering?

**Domain**: Authorization design, security architecture, access control models

**Established Context** (from prior discussion):

- **Person (Keeper)**: Identity authenticated via credentials (egregore pubkey, discord ID, bearer token)
- **Place**: Where the request originates (`discord:guild:channel`, `egregore:*`, `http:*`, `a2a:agent-name`)
- **Skill**: What capability is invoked (`shell:execute`, `docker:*`, etc.)

**Key Observation**: User cannot articulate a use case where they would allow a Keeper via one channel but deny via another.

## Problem Decomposition

### Atomic Tasks for Analysis

| # | Task | Question to Answer |
|---|------|-------------------|
| 1 | Define authorization dimensions | What are Person/Place/Skill precisely in authorization theory? |
| 2 | Identify Place's unique contribution | What does Place enable that Person+Skill cannot? |
| 3 | Enumerate concrete use cases | When would Place-based restrictions matter? |
| 4 | Assess implementation cost | What complexity does Place add to the system? |
| 5 | Evaluate audit vs authorization | Is Place valuable for logging even if not for authz? |
| 6 | Compare to industry models | How do other systems handle channel-based authz? |
| 7 | Risk analysis | What threats does Place mitigate (or fail to mitigate)? |

### Existing Coverage Analysis

| Task | Agent Candidate | Score | Rationale |
|------|-----------------|-------|-----------|
| 1-2 | security-auditor | 8/10 | Strong on security frameworks, access control theory |
| 3-4 | devops-engineer | 7/10 | Practical deployment experience, complexity assessment |
| 5 | security-auditor | 8/10 | Audit logging in SOC 2/compliance frameworks |
| 6 | research-assistant | 7/10 | Can survey OAuth, RBAC, ABAC literature |
| 7 | security-auditor | 9/10 | Threat modeling, risk assessment core competency |

### Gap Analysis

**Full coverage (score >= 7)**: All tasks covered by existing agents.
**ROI Assessment**: No new agent required.

## Required Expertise Areas

1. **Security Architecture** - Access control models (RBAC, ABAC, ReBAC), threat modeling
2. **Practical Operations** - Real-world deployment patterns, complexity/maintenance burden
3. **Authorization Theory** - Theoretical grounding for multi-dimensional access control
4. **Skeptical Adversary** - Challenge assumptions, steelman counter-positions

## Recruited Specialists

### 1. security-auditor --> "The Architect" (score: 9/10)

**Session Role**: Security architecture and access control theory specialist

**Why Selected**: Deep expertise in security frameworks (SOC 2, NIST, ISO 27001), access control evaluation, and threat modeling. Can assess whether Place provides genuine security value or is security theater.

**Panel Focus**:

- Theoretical grounding for Place as authorization dimension
- Threat model analysis: what attacks does Place prevent?
- Industry precedent for channel-based access control
- Audit vs authorization distinction

---

### 2. devops-engineer --> "The Operator" (score: 7/10)

**Session Role**: Practical deployment and operational complexity specialist

**Why Selected**: Real-world experience deploying and maintaining complex systems. Can assess implementation cost, configuration burden, and whether Place adds operational value or operational friction.

**Panel Focus**:

- Implementation complexity assessment
- Configuration maintenance burden
- Practical use cases from deployment experience
- Simplicity vs flexibility tradeoffs

---

### 3. research-assistant --> "The Scholar" (score: 7/10)

**Session Role**: External research and industry comparison specialist

**Why Selected**: Can survey authorization literature, compare to industry standards (OAuth, OIDC, SPIFFE/SPIRE), and bring external evidence to inform the decision.

**Panel Focus**:

- Survey of multi-dimensional authorization in industry
- Academic/practitioner perspectives on channel-based access
- Precedent from similar systems (HashiCorp Boundary, Teleport, etc.)

---

### 4. Adversarial Role: "The Skeptic" (analyst self-assignment)

**Session Role**: Challenge assumptions, steelman both positions

**Why Selected**: Panel needs someone to actively challenge the "Place is useful" position AND the "Place is over-engineering" position. Prevents groupthink.

**Panel Focus**:

- Steelman: strongest case FOR Place
- Steelman: strongest case AGAINST Place
- Identify hidden assumptions in both positions

## Inferred Panel Goals

1. **Determine if Place provides authorization value** (not just audit value)
2. **Identify concrete scenarios** where Place-based denial makes sense
3. **Assess implementation cost** vs. security benefit
4. **Reach consensus** on whether to keep, simplify, or remove Place

## Decision Rule

**Consensus** - This is an architectural decision affecting security model coherence. All specialists should agree on the recommendation, or dissent should be documented with clear rationale.

## Evidence Already Available

From `authority.example.toml`:

```toml
# Limited automation access -- only from local egregore, only docker inspect
[[permission]]
keeper = "automation"
place = "egregore:local"
skills = ["docker:inspect_*", "docker:logs"]

# Bot service can only use shell from Discord guild
[[permission]]
keeper = "bot-service"
place = "discord:187489110150086656:*"
skills = ["shell:*"]
```

**Observation**: These examples use Place to restrict *where* automation or bots can originate requests. But is this meaningful if the bot-service token only exists in Discord anyway?

## Key Questions for Panel

1. **Redundancy test**: If a Keeper identity (e.g., discord user ID) is inherently tied to a Place (Discord), does explicit Place checking add value?

2. **Cross-channel identity**: Could a Keeper have the same identity across multiple Places? If not, Place is redundant with Person.

3. **Privilege escalation**: Does Place prevent a compromised channel from escalating via another channel? Or is this better handled by separate Keeper identities?

4. **Audit sufficiency**: Would logging Place without authorization checks provide the same security insight at lower complexity?

5. **Future-proofing**: Does Place enable scenarios not yet implemented (e.g., same human via Discord vs egregore with different trust levels)?

## Panel Schedule

| Phase | Description | Specialist Focus |
|-------|-------------|------------------|
| 01 | Framing | Moderator restates topic, goals, decision rule |
| 02 | Initial Positions | Each specialist provides 5-bullet brief |
| 03 | Cross-Examination | Adversarial challenges to positions |
| 04 | Research Gate | Scholar provides external evidence if needed |
| 05 | Synthesis | Options and tradeoffs summary |
| 06 | Decision | Apply consensus rule, document outcome |
