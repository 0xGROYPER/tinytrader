# Job queue

| | |
|---|---|
| **Purpose** | Run the deployment's own batch work (research scripts, collectors) from templates, in Docker or on a host runner, after dependencies, at a time, after the close, or only while capture is idle ([decision 0012](../decisions/0012-job-queue.md)). |
| **Transport** | HTTP/1.1, JSON, on `QUEUE_LISTEN` (default `127.0.0.1:9123`). The `tt` command line is a client of it (`TT_QUEUE`). |
| **Versioning** | Templates are `apiVersion: tinytrader/v1`, `kind: JobTemplate`. Paths are unversioned; additive changes only. |
| **Auth** | Writes are refused from a web page of another origin. No token yet: anyone who can reach the port can add jobs (from templates only) and act as a host runner, so bind it to loopback or a private network ([decision 0017](../decisions/0017-client-contracts.md)). |
| **Errors** | `{"error": "..."}`: `404` unknown job or template, `409` the job's state doesn't allow it, `422` a parameter or dependency problem. |
| **Health** | [Observe](observe.md) endpoints and `/events`. |
| **Stability** | **Experimental.** Job JSON is camelCase (it follows the Kubernetes-style templates), unlike the other services. |

## Configuration

| Variable | |
|---|---|
| `QUEUE_TEMPLATES` | Required: the YAML file of templates. |
| `QUEUE_STATE` | Required: where `jobs.jsonl` (the journal) and logs are kept. |
| `QUEUE_DOCKER` | Docker Engine API (a containers-only proxy); without it Docker jobs wait. `QUEUE_NETWORK`: their network. |
| `QUEUE_MAX_RUNNING` | Docker jobs at once (default 2). |
| `QUEUE_YIELD_TO` | `NAME=OBSERVE_URL;...`: services whose `/health` state other than `idle` means "capture is live"; `background-may-run` jobs wait while any is. |
| `QUEUE_CALENDAR` | A market calendar URL (`{"days": [{date, open, close}]}`, such as [alpaca-account's](alpaca-account.md#get-calendar)), for `after-close`. Without it, weekdays close at 16:00 New York. |
| `QUEUE_SECRETS`, `QUEUE_SECRETS_DIR`, `QUEUE_SECRETS_VOLUME` | Secrets copied for jobs to mount. |

## Jobs

```json
{"id": "collect-7", "template": "collect", "runner": "docker", "params": {"script": "collect.py"},
 "command": ["python", "collect.py"], "args": [], "when": "after-close", "after": [],
 "state": "Waiting", "reason": "after the close", "created": 1791200000.5, "cancelRequested": false, "addedBy": "tt"}
```

`state` is `Waiting`, `Pending`, `Running`, `Succeeded`, `Failed` or `Cancelled`; `when` is `anytime`, `after-close` or `background-may-run`. `created`, `started`, `finished` and `notBefore` are Unix seconds.

| Endpoint | |
|---|---|
| `GET /jobs` | `{"jobs": [...], "updated_at"}`: open jobs, then the 100 most recently finished. |
| `GET /templates` | `{"templates": [{name, description, runner, class, parameters, command, image, workingDir, activeDeadlineSeconds}]}` |
| `POST /jobs` | `{template, params?, notBefore?, when?, after?, addedBy?}` → `201` with the job. |
| `GET /jobs/{id}` | The job. |
| `PATCH /jobs/{id}` | `{notBefore?, when?, after?}`, while it waits. |
| `POST /jobs/{id}/cancel` | Cancel it, or stop it if running. |
| `POST /jobs/{id}/run-now` | Drop its conditions. |
| `GET /jobs/{id}/log?tail=N` | `{id, state, log}`. |

A host runner (`queue host-runner`, with `QUEUE_URL`) uses `POST /claim` (`{runner}` → `204` or `{job, workingDir, env, activeDeadlineSeconds}`), `POST /jobs/{id}/log` (raw bytes) and `POST /jobs/{id}/finish` (`{exitCode}`).

## Templates

```yaml
apiVersion: tinytrader/v1
kind: JobTemplate
metadata: { name: collect, description: Run a collector script }
spec:
  runner: docker
  class: background
  parameters: { script: { pattern: "[a-z_]+\\.py" } }
  template:
    spec:
      containers:
        - image: my-research:latest
          command: [python, "{script}"]
          resources: { limits: { cpu: "1", memory: 1Gi } }
```

`{name}` slots are filled from validated parameters in `command` and `args` only; `${VAR}` is expanded from the queue's environment when the templates load.
