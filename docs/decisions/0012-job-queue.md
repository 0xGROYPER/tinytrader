# 0012: A job queue, shaped like Kubernetes Jobs

- **Status:** Accepted
- **Date:** 2026-10-08
- **Builds on:** [0011](0011-bundles-and-work-classes.md) (work classes)

## Context

Work that should run later has no home. On 2026-10-08 two research
collectors were queued for after the close as `tmux` sessions that sleep
until 16:20, and an earlier timer quietly expired before its time. Nobody can
see that work, edit it or cancel it, except by finding the sessions. Builds
and backtests on the same machine have the same problem, and 0011 gave them a
rule (background work yields to live capture) but no place to wait.

## Decision

tinytrader gets a **queue service** and two **runners**, and the console a
**Queue widget**. The shapes follow Kubernetes' `Job`, so moving to a cluster
later is a translation, not a redesign. Kubernetes itself is not adopted now:
on one 8 GB machine its control plane would cost the memory and CPU the live
feeds need, and the planned VPS and NAS run NixOS with systemd.

### Job templates: the only work that can run

A **job template** is defined by the deployment, never by a request. It names
a runner and everything the work needs, in the fields Kubernetes uses:

```yaml
apiVersion: tinytrader/v1
kind: JobTemplate
metadata: { name: research-collector }
spec:
  runner: docker                       # or: host
  class: background                    # 0011
  parameters:                          # the only values a request may give
    script: { pattern: '^collect_[a-z_]+\.py$' }
  activeDeadlineSeconds: 7200
  backoffLimit: 0
  template:
    spec:
      containers:
        - image: tinytrader-ops/research:local
          command: [python, '{script}']
          workingDir: /work
          resources: { limits: { cpu: '1', memory: 1Gi } }
          volumeMounts: [{ name: research, mountPath: /work }]
          envFrom: [{ secretRef: { name: databento } }]
      volumes: [{ name: research, hostPath: { path: '${TINYTRADER_OPS}/research' } }]
```

A **job** is a template plus parameter values and **when it may start**:

- `notBefore`: a time;
- `after`: other jobs that must have succeeded;
- `when`: `anytime`, `after-close` (the market calendar's close on trading
  days, any time otherwise), or `background-may-run` (0011: no capture
  service live, not under pressure).

A job's states are Kubernetes' plus waiting: `Waiting`, `Pending`, `Running`,
`Succeeded`, `Failed`, `Cancelled`. Parameters are checked against the
template's patterns; anything else is refused.

### The queue service

- Holds templates (read from the deployment) and jobs (an append-only journal
  on disk, so a restart loses nothing and never starts a job twice).
- Starts a job when its conditions hold, on the runner its template names.
- Serves the queue: list, add, cancel, edit `notBefore` / `when` / `after` of
  a waiting job, run now (clears the conditions), and each job's log.
- Reports through `tinytrader.observe/1` like every service.

### Runners

- **docker** (the queue service itself): each job is a one-off container
  created from its template, with the template's limits and a low CPU share,
  labelled `tinytrader.job=<id>`, removed after its log is kept. Cancel stops
  it. This needs write access to Docker, which nothing in tinytrader had: the
  queue service gets its own Docker proxy allowing containers (create, start,
  stop, logs, remove) and nothing else (no exec, images, networks, volumes,
  builds). The proxy cannot restrict what a created container may mount or
  whether it is privileged, so **the queue service is the gate**: it builds
  every request from a template and never from client input.
- **host**: a small agent on the Mac (a launchd agent from the nix-darwin
  module) that claims host jobs from the queue and runs the template's
  command under `taskpolicy -b`, for builds and backtests that can't run in
  Docker. It runs only commands its templates define, in the checkouts they
  name.

### Clients

- The **Queue widget**: waiting, running and finished jobs, with conditions,
  durations and logs; cancel, edit, run now, and add from the templates.
- `tt queue add TEMPLATE key=value … [--not-before T] [--when …] [--after ID]`,
  `tt queue list`, `tt queue cancel ID`: so agents enqueue work where the
  operator sees it, instead of starting timers of their own.

### Mapping to Kubernetes

| Here | Kubernetes |
|---|---|
| Job template | `Job` spec (`template`, `activeDeadlineSeconds`, `backoffLimit`) |
| `when: background-may-run` | a controller gate (no built-in equivalent) |
| `notBefore` | a suspended `Job` released at the time (`spec.suspend`) |
| docker runner | the kubelet |
| host runner | a node outside the cluster (no equivalent; stays a host agent) |

## Plan

1. The queue service with the docker runner, its own Docker proxy, the
   journal, the API and `tt queue`.
2. The Queue widget.
3. The host runner and its nix-darwin module.
4. The deployment's first templates: research collectors and notebooks
   (docker), tinytrader builds and image builds (host).

## Consequences

- Deferred work is visible and editable in one place, and agents use the
  same queue as the operator.
- Background work waits for 0011's rule by itself instead of by
  convention.
- tinytrader holds Docker write access for the first time, narrowed to
  containers, with templates as the boundary.
- No general workflow engine: no retries beyond `backoffLimit`, no
  schedules that repeat (a repeating job is a later `CronJob`-shaped
  addition), no fan-out.
