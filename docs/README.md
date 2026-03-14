# Servitor Docs

This directory describes the current `servitor` runtime as shipped on the
branch this docs set tracks.

- Protocol and message lifecycle: [protocol.md](protocol.md)
- Configuration reference: [configuration.md](configuration.md)
- Operational guidance: [operations.md](operations.md)
- HTTP and OpenAPI boundary: [api/README.md](api/README.md)

Important current-state notes:

- `servitor` has a documented egregore message protocol, not a standalone
  public HTTP control plane.
- `egregore.group` and `comms.http` are parsed config sections but are not
  wired into the runtime on this branch.
- `task_claim` still exists for direct, hook, cron, and watcher executions, but
  coordinated SSE work uses the newer offer/assign lifecycle.
