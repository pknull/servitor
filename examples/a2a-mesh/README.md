# A2A Mesh Example

Multi-node Egregore + Servitor cluster with A2A agent-to-agent communication.

## Architecture

```
                    ┌─────────────────────────────────────────────┐
                    │              Docker Network                  │
                    │                                              │
  ┌─────────────────┼──────────────────────────────────────────────┼─────────────────┐
  │                 │                                              │                 │
  │  ┌──────────────▼───────────┐  gossip   ┌──────────────────────▼───────────┐    │
  │  │       egregore-1         │◄─────────►│       egregore-2                 │    │
  │  │   :7654 (API)            │           │   :7654 (API)                    │    │
  │  │   :7655 (gossip)         │           │   :7655 (gossip)                 │    │
  │  └───────────┬──────────────┘           └──────────────┬───────────────────┘    │
  │              │                                          │                        │
  │              │ subscribe                                │ subscribe              │
  │              │                                          │                        │
  │  ┌───────────▼──────────────┐   A2A     ┌──────────────▼───────────────────┐    │
  │  │       servitor-1         │──────────►│       servitor-2                 │    │
  │  │   :8765 (A2A server)     │           │   :8765 (A2A server)             │    │
  │  │   - echo (MCP)           │           │   - echo (MCP)                   │    │
  │  │   - servitor-2 (A2A)     │           │   - servitor-3 (A2A)             │    │
  │  └──────────────────────────┘           └──────────────┬───────────────────┘    │
  │                                                         │                        │
  │                                                         │ A2A                    │
  │                                                         │                        │
  │                      ┌──────────────────────────────────▼───────────────────┐   │
  │                      │       egregore-3 ◄──► servitor-3                     │   │
  │                      │   - echo (MCP)                                       │   │
  │                      │   - NO outbound A2A (terminal node)                  │   │
  │                      └──────────────────────────────────────────────────────┘   │
  │                                                                                  │
  └──────────────────────────────────────────────────────────────────────────────────┘
```

## Loop Prevention Strategy

This example uses **DAG topology** for loop prevention:

```
servitor-1 → servitor-2 → servitor-3 → (terminates)
```

Each servitor can only delegate to the **next** node in the chain. Servitor-3 has no outbound A2A connections, terminating any delegation chain.

### Alternative Loop Prevention Strategies

For production deployments with arbitrary topologies, implement one of:

| Strategy | Implementation | Pros | Cons |
|----------|----------------|------|------|
| **Hop Count** | Pass `X-A2A-Hop-Count` header, decrement on each delegation, reject at 0 | Simple, O(1) check | Requires protocol extension |
| **Task ID Set** | Track seen task IDs in header, reject if ID already in set | Detects exact loops | Header grows with chain length |
| **Origin Tracking** | Include origin servitor ID, reject delegation back to origin | Prevents direct loops | Doesn't prevent A→B→C→A |
| **Circuit Breaker** | Already implemented per-agent; 3 failures → open state | Handles cascading failures | Not loop-specific |

### Implementing Hop Count (Recommended)

Add to `src/a2a/client.rs`:

```rust
// In A2A client request
fn build_request(&self, skill: &str, input: Value, hop_count: u8) -> Request {
    Request::builder()
        .header("X-A2A-Hop-Count", hop_count.to_string())
        // ...
}
```

Add to `src/a2a/server/handlers.rs`:

```rust
// In handle_tasks_send
let hop_count: u8 = headers
    .get("X-A2A-Hop-Count")
    .and_then(|v| v.to_str().ok())
    .and_then(|s| s.parse().ok())
    .unwrap_or(5);  // Default max hops

if hop_count == 0 {
    return JsonRpcResponse::error(id, -32003, "Max delegation depth exceeded");
}

// Pass hop_count - 1 to any subsequent A2A delegations
```

## Usage

```bash
# Start the mesh
cd examples/a2a-mesh
docker compose up -d

# Check health
docker compose ps

# View logs
docker compose logs -f servitor-1

# Test A2A discovery
curl http://localhost:8765/.well-known/agent.json | jq

# Submit task to servitor-1
curl -X POST http://localhost:8765/a2a \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tasks/send","params":{"skill":"echo_execute","input":{}}}'

# Check egregore gossip
curl http://localhost:7664/status | jq

# Stop
docker compose down -v
```

## Prerequisites

- Docker with Compose v2
- ~8GB RAM for Ollama + services
- Build contexts: egregore and servitor repos

## Configuration Files

| File | Purpose |
|------|---------|
| `config/egregore-*.yaml` | Egregore node configs with shared network_key |
| `config/servitor-*.toml` | Servitor configs with A2A server/client settings |
| `config/authority.toml` | Shared authority (open mode for testing) |

## Ports

| Service | Internal | External |
|---------|----------|----------|
| egregore-1 API | 7654 | 7664 |
| egregore-2 API | 7654 | 7674 |
| egregore-3 API | 7654 | 7684 |
| servitor-1 A2A | 8765 | 8765 |
| servitor-2 A2A | 8765 | 8775 |
| servitor-3 A2A | 8765 | 8785 |
| Ollama | 11434 | 11434 |
