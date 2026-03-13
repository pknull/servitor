# Containerized Servitor With MCP Sidecars

This deployment pattern keeps `servitor` itself away from direct host access while still allowing specific tools through isolated MCP sidecars.

## Pattern

```text
┌─────────────────┐     HTTP      ┌─────────────────┐
│    Servitor     │──────────────▶│  MCP: shell     │──▶ /workspace only
│   (no host)     │               │  (volume mount) │
└─────────────────┘               └─────────────────┘
         │
         │ HTTP
         ▼
┌─────────────────┐
│  MCP: docker    │──▶ docker.sock
│  (socket mount) │
└─────────────────┘
```

## Why This Layout

1. The `servitor` process does not need host filesystem mounts.
2. Each MCP server gets only the mount or socket it actually needs.
3. Scope policy still limits tool use inside each sidecar.
4. Container boundaries reduce the blast radius of a compromised tool or bad prompt.

## Servitor Configuration

Configure Servitor to talk to the sidecars over HTTP:

```toml
[mcp.shell]
transport = "http"
base_url = "http://mcp-shell:3000/mcp"
scope.allow = ["execute:/workspace/**"]
scope.block = ["execute:/workspace/.git/**", "execute:rm *"]

[mcp.docker]
transport = "http"
base_url = "http://mcp-docker:3000/mcp"
scope.allow = ["container_list", "container_logs", "image_list"]
```

## Docker Compose Example

See [examples/containerized/docker-compose.yml](../../examples/containerized/docker-compose.yml).

The example assumes:

1. `servitor` runs with a read-only root filesystem and only its state directory writable.
2. The shell MCP sidecar mounts only the workspace it is allowed to touch.
3. The Docker MCP sidecar mounts only the Docker socket.
4. Sidecars live on a private bridge network and are not published externally.

## systemd Example

See:

- [examples/systemd/servitor.service](../../examples/systemd/servitor.service)
- [examples/systemd/servitor-shell-mcp.service](../../examples/systemd/servitor-shell-mcp.service)
- [examples/systemd/servitor-docker-mcp.service](../../examples/systemd/servitor-docker-mcp.service)

Recommended hardening flags for the `servitor` unit:

1. `PrivateTmp=yes`
2. `NoNewPrivileges=yes`
3. `ProtectSystem=strict`
4. `ProtectHome=yes`
5. `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`
6. `ReadWritePaths=` only for the state directory

## Security Considerations

1. Do not mount the host filesystem into the `servitor` container.
2. Treat each sidecar as a separate trust boundary and give it the minimum mount set possible.
3. Keep dangerous tools out of `scope.allow` even when the sidecar is isolated.
4. Prefer read-only mounts unless a tool absolutely requires writes.
5. If a sidecar needs a privileged socket such as `/var/run/docker.sock`, isolate it into its own service and restrict Servitor's scope to low-risk Docker operations.
6. Keep the sidecar network private. Only expose Servitor itself if you explicitly need remote access.
