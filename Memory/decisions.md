# Decisions

- Servitor carries no LLM. Tasks arrive with concrete tool_calls or are
  rejected; Familiar owns all reasoning and planning.
- RFC 0003 binds execution authority here: Servitor authorizes every
  assignment and owns every task state transition; typed commands are
  concrete tagged types with layered idempotency and causal record hashes.
- Fail closed: daemon and hook modes refuse work without authority.toml
  unless --insecure is set explicitly for local development; empty
  allowlists mean none. Skill wildcards never cross a ':' segment boundary.
- Results are signed and published only through the local Egregore node.
- Public repo on a single protected-by-convention main; CI (format, clippy,
  test, build) must be green before pushing.
- This repo carries its own Memory v2 pair (activeContext.md, decisions.md)
  under the Control workspace contract; cross-component decisions live in
  the Thallus umbrella. Machine-local state stays under ignored Work/,
  memory-local/ and .servitor/.
