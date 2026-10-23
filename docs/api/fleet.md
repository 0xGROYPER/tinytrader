# Fleet: discovery, the topology file and `tinytrader.fleet/1`

| | |
|---|---|
| **Purpose** | How front ends find a deployment's services and what they see: Docker labels and the topology file going in, the topology and state documents coming out. Implemented once in [`libs/fleet`](../../libs/fleet/src/lib.rs) ([decision 0003](../decisions/0003-front-ends.md)); the console and the terminal UI share it. |
| **Transport** | Inputs: container labels and a JSON file. Outputs: the console's `/api/topology` and `/api/state` ([console API](console.md)), or `Poller` in-process. |
| **Versioning** | `"schema": "tinytrader.fleet/1"` in both output documents. The topology file has no schema field; it changes only by gaining optional fields. |
| **Auth** | None. Docker is reached through `CONSOLE_DOCKER` (ideally a read-only socket proxy). |
| **Errors** | An unreadable topology file is reported in `/api/state`'s `error`; Docker trouble in `docker.error`. |
| **Health** | Each service's `/health` result is in `/api/state`. |
| **Stability** | **Stable**: the labels, the topology file fields below, and the documents' fields. Every field is optional on the wire. |

## Docker labels

A container is drawn when it's in the Compose project the console is told about (`CONSOLE_COMPOSE_PROJECT`, optional: without it, every container the Docker API lists). Its labels:

| Label | | |
|---|---|---|
| `tinytrader.observe` | `"<port>"` | The port of its [observe endpoints](observe.md). The console asks `http://<name>:<port>/describe`, where `<name>` is the Compose service name (else the container name), so the console must share a network with it. |
| `tinytrader.label` | text | The name to show (default: the service name). |
| `tinytrader.role` | text | One line on what it does (else `/describe`'s `role`). |
| `tinytrader.host` | host id | Which host it's drawn on (default `local`). |

```yaml
services:
  relay:
    labels: { tinytrader.observe: "9110", tinytrader.label: "Stocks relay" }
```

Docker discovery is a convenience, not a requirement: anything listed by address in the topology file is found the same way.

## The topology file

`CONSOLE_TOPOLOGY` (default `topology.json`; the terminal UI's `--topology`): optional overrides layered over what was discovered. On NixOS and nix-darwin the `tinytrader.services` module writes it. Every field is optional:

```json
{
  "name": "my deployment",
  "focus": "relay-stocks",
  "hosts": [{ "id": "local", "label": "this machine", "exporter": "http://127.0.0.1:9100/metrics" }],
  "services": [
    { "id": "ticker", "observe": "http://127.0.0.1:9105" },
    { "id": "relay-stocks", "label": "Stocks relay" }
  ],
  "external": [{ "id": "vendor", "label": "Vendor", "sub": "websocket" }],
  "stores": [],
  "edges": [{ "from": "vendor", "to": "ticker", "kind": "flow" }],
  "panels": [],
  "bundles": [],
  "links": { "notebook": "http://127.0.0.1:8888" },
  "hide": ["socket-proxy"]
}
```

| Field | Effect |
|---|---|
| `name` | The deployment's name (default: the Compose project, else `deployment`). |
| `focus` | The service front ends show first (the terminal UI orders relays by it). |
| `hosts` | Replace the hosts: `{id, label, sub, exporter}`; `exporter` is a node_exporter metrics URL. |
| `services` | Merged by `id` over discovered services, field by field. An entry with `observe` (a base URL) is **discovered by address**: asked to `/describe` itself whatever runs it. Any discovered field can be set or replaced (`label`, `role`, `host`, `health`, `metrics`, a capability URL such as `chain`). |
| `external`, `stores` | Merged by `id`: sources outside the deployment and places data is kept. |
| `edges` | Added: `{from, to, kind, label}`, `kind` `flow` or `read`. |
| `panels` | Replace the panels `/describe` suggested for the services they name. |
| `bundles` | Groups of services with shared hours and a work class ([decision 0011](../decisions/0011-bundles-and-work-classes.md)). |
| `links` | Tools run beside tinytrader, by name: `notebook` and `terminal` are framed by their widgets; every `http(s)` link is listed in the [Services widget](../widgets/services.md), such as a batch scheduler's UI. |
| `hide` | Ids to leave out, with their edges and panels. |

## `/api/topology`

The deployment as drawn: `{schema, name, focus, hosts, external, services, stores, edges, panels, bundles, links}`. Each service:

| Field | |
|---|---|
| `id`, `label`, `role`, `host` | |
| `health`, `metrics` | Its observe URLs. |
| `events`, `recent`, `catalog`, `news`, `accounts`, `calendar`, `econ`, `earnings`, `clients` | URLs of what it declared in `/describe`'s `streams`, polled into `/api/state`. |
| `bars`, `chain`, `snapshots`, ... | URLs of on-demand streams, fetched through [`/api/stream/{service}/{stream}`](console.md#data-from-services). |
| `relay` | A relay's `{protocol, listen, symbols}`. |
| `schedule` | What it runs and when. |

**Find services by capability, never by id.** A front end looks for the first service with the field it needs (`services.find(s => s.chain)`; in the console page, `serviceFor('chain')` and `servicesFor('accounts')` in `web/lib/fleet.js`); ids are the deployment's own choice.

## `/api/state`

The latest poll (every 2 seconds): `{schema, generated_at, hosts, docker: {configured, error, stale_seconds}, services: {id: {container, health, metrics, recent, catalog, accounts, calendar, news, econ, earnings, clients}}}`.

- `health` is `{http_status, body, stale_seconds?}` or `{error}`; `body` is the service's [`/health`](observe.md#get-health) document.
- `metrics` is the parsed Prometheus text: `{name: [{labels, value}]}`.
- `generated_at` is Unix seconds.

The typed form is `Topology`, `Snapshot` and `EventsPage` in `libs/fleet`; a terminal front end can embed a `Poller` or read a running console (`Source::Console`).
