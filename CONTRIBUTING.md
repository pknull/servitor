# Contributing to Servitor

Thanks for your interest. Servitor is the executor of [Thallus](../) — it receives pre-planned tool calls from Familiar and runs them against MCP servers, publishing results back to egregore through the local node.

## Before You Start

- Read the main project's [CLAUDE.md](CLAUDE.md) for module layout and architecture.
- For authority/scope model changes, read `authority.example.toml` and `src/authority/mod.rs`.
- For large changes, open an issue first to discuss.

## Development Setup

```bash
git clone <repo>
cd servitor
cargo build
cargo test
```

Stable Rust toolchain only.

## Running Servitor

```bash
./target/release/servitor init     # Generate identity and default config
./target/release/servitor info     # Verify config + authority
./target/release/servitor run      # Daemon mode
```

For testing without authority (development only):

```bash
SERVITOR_INSECURE=1 ./target/release/servitor run --insecure
```

## Pre-Submit Checklist

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release
```

CI also runs `cargo audit`. Install once: `cargo install cargo-audit`.

## Areas That Need Care

### Authority and Scope
`src/authority/` and `src/scope/` are security-critical. Changes here require tests that cover both allow and deny paths. Block takes precedence over allow — preserve that invariant.

### Output Defense
`src/agent/output_defense.rs` is the last line of defense against tool output attacks (credential leaks, prompt injection into feed messages). Add regression tests for any pattern you fix.

### No LLM in Servitor
Servitor is a **pure executor**. It does not call LLMs. All planning happens in Familiar. If your change requires reasoning at execution time, it belongs in Familiar, not Servitor.

## Code Style

- Rust 2021 edition
- `cargo fmt` defaults, `cargo clippy --all-targets`
- `thiserror` for library errors, `anyhow` in binaries
- Async with tokio
- Doc comments on public APIs

## What's Deferred

These are called out in CLAUDE.md under "Implementation Status":
- Consumer groups
- Capability challenges
- Reputation tracking

Don't start on these without discussing first — they may be out of scope for current direction.

## Pull Request Process

1. Fork and branch from `master`
2. Make your change; add tests
3. Run the pre-submit checklist
4. Open a PR with a clear description
5. Solo maintainer — review turnaround varies

## License

By contributing, you agree that your contributions will be licensed under [MIT OR Apache-2.0](../LICENSE-MIT).
