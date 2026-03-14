# Servitor HTTP and OpenAPI Boundary

Servitor does not currently expose a stable inbound HTTP control API of its own,
so this repository does not ship a Servitor OpenAPI document on this branch.

That is intentional and reflects the actual runtime boundary:

- inbound control surfaces are CLI, egregore messages, hook stdin, cron,
  watchers, and Discord
- outbound HTTP is used to talk to egregore and to MCP servers configured with
  `transport = "http"`

## What Servitor Calls

Servitor acts as a client of the egregore node API, primarily:

- `POST /v1/publish`
- `GET /v1/events`

Those HTTP contracts belong to the `egregore` repository and are documented in
its OpenAPI file at `egregore/docs/api/node-api.yaml`.

## What Servitor Does Not Currently Expose

- no public REST control plane
- no documented localhost management API
- no inbound webhook server wired into the runtime

The `comms.http` configuration block exists in the schema, but it is currently a
reserved surface rather than an instantiated transport. It should not be treated
as a stable API contract yet.
