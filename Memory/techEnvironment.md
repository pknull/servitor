---
version: "1.0"
lastUpdated: "2026-03-05"
lifecycle: "active"
stakeholder: "technical"
changeTrigger: "Stack changes, dependency updates"
dependencies: []
---

# Technical Environment

## Language & Toolchain

- **Language**: Rust (2021 edition)
- **Build**: Cargo
- **Minimum Rust**: 1.75+ (async trait support)

## Key Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| tokio | 1 | Async runtime |
| serde | 1 | Serialization |
| reqwest | 0.12 | HTTP client |
| ed25519-dalek | 2 | Cryptography |
| clap | 4 | CLI parsing |
| tracing | 0.1 | Logging |

## Project Structure

```
servitor/
├── src/
│   ├── main.rs          # CLI entry point
│   ├── lib.rs           # Library exports
│   ├── error.rs         # Error types
│   ├── config/          # TOML configuration
│   ├── identity/        # Ed25519 keys
│   ├── egregore/        # Network integration
│   ├── mcp/             # MCP client pool
│   ├── scope/           # Policy enforcement
│   └── agent/           # LLM + tool loop
├── tests/
│   └── integration.rs   # Integration tests
├── Memory/              # Asha memory bank
└── servitor.example.toml
```

## Build Commands

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Check without building
cargo check

# Format code
cargo fmt

# Lint
cargo clippy
```

## Environment Variables

| Variable | Required | Purpose |
|----------|----------|---------|
| ANTHROPIC_API_KEY | For Anthropic provider | Claude API access |
| OPENAI_API_KEY | For OpenAI provider | GPT API access |

## Configuration

Config file: `servitor.toml` (see `servitor.example.toml`)

Key sections:

- `[identity]`: Data directory for keys
- `[egregore]`: API endpoint
- `[llm]`: Provider, model, credentials
- `[mcp.*]`: MCP server definitions with scope
- `[agent]`: Execution parameters

## Related Projects

- `~/Code/egregore`: Network daemon (identity patterns, hooks)
- MCP servers: Tool capabilities
