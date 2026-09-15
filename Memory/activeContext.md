# Objective

Servitor is the hands of Thallus: a headless executor that receives
pre-planned tool_calls, enforces authority and scope per call against its MCP
pool (and A2A agents), and publishes signed task results through the local
Egregore node. It owns assignment authorization and every task state
transition under RFC 0003. It carries no LLM.

# State

Verified 2026-09-15. Version 0.3.0; main at cdfcc6f7 (public repo
pknull/servitor); CI green on main. cdfcc6f7 fixed the authorization bypass
where an underscore-glob grant such as docker:inspect_* also matched a deeper
skill segment (docker:inspect_container:privileged); skill_pattern_matches now
requires equal ':' segment counts, with regression tests. 166 tests pass
(cargo test). AssignTaskV1 typed assignment (d915bee) and the 2026-08-15
documentation audit are landed. This repo now carries its own Memory v2 pair
plus the Control workspace contract (.asha/workspace.json, knowledge/ index)
from the Control bootstrap; cross-component decisions stay in the Thallus
umbrella (private repo pknull/Thallus). Local servitor.toml, .servitor/
identity and Work/ are ignored.

# Next

- Webhook target conformance (RFC 0003 §6 conditions) before webhooks lose
  their compatibility-only label.
- Keep authority pattern matching covered by nested-namespace negative tests
  whenever a new wildcard form is added.

# Blockers

- None.
