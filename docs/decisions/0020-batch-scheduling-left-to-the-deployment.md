# 0020: Batch scheduling is left to the deployment

- **Status:** Accepted
- **Date:** 2026-10-23
- **Supersedes:** [0012](0012-job-queue.md) (the job queue)
- **Amends:** [0011](0011-bundles-and-work-classes.md) (background work yields
  to live capture: still the rule, now kept by the deployment's scheduler)

## Context

0012 gave tinytrader its own job queue: a service of about 2,300 lines of
Rust, with job templates, a journal, a Docker runner behind its own Docker
proxy, a host runner, a console widget and the `tt queue` command. It works,
but it is a small, general workload scheduler, and general schedulers exist
already: they place containers and host commands by resources, run batch and
periodic jobs, keep logs and history, and come with a UI and a command line.

Nothing in the queue was specific to trading. Its conditions were generic:
a time (`notBefore`), other jobs (`after`), the market calendar's close
(`after-close`), and "wait while these services' `/health` isn't `idle`"
(`background-may-run`, 0011). Those are a calendar URL and a health URL,
both already plain HTTP contracts of the platform
([alpaca-account's `/calendar`](../api/alpaca-account.md#get-calendar),
[observe's `/health`](../api/observe.md)), which any scheduler can wait on
with a few lines of shell.

Keeping a scheduler of our own means keeping its Docker write access (0012's
largest risk), its proxy, its state format and its UI, all for one
deployment's handful of templates.

## Decision

tinytrader has no job queue. **Batch work is scheduled by the deployment**,
with whatever scheduler it already runs: HashiCorp Nomad, Kubernetes Jobs and
CronJobs, systemd timers, or cron and `docker run`. The platform's part is
what any of them can use:

- **Containers are the unit of work.** tinytrader's images and the
  deployment's own are ordinary OCI images; a job is a container (or a host
  command) with resource limits.
- **Conditions are HTTP checks a job runs before its work**, as a first step
  (a Kubernetes init container, a Nomad prestart task, an `ExecStartPre`):
  - *after the close*: read a market calendar (`{"days": [{date, open,
    close}]}`, New York times) and wait until today's close has passed;
  - *yield to capture* (0011): wait until each named service's `/health`
    reports `state: "idle"`, the observe contract's "off on purpose";
  - *at a time*, *after another job*: the scheduler's own (a schedule in
    `America/New_York`, job dependencies or ordered tasks).
- **No secret a job doesn't need.** A job gets what its spec names; keys stay
  with the services that hold them (provider purchases go through
  databento-jobs, which keeps its own guards: [0019](0019-purchase-guards.md)).

Removed: `services/queue` (the queue service, its Docker and host runners
and the `tt` command), its API document, the console's `/api/queue` proxy
and its Queue widget. A deployment that wants its scheduler's UI in the
console adds it to the topology file's `links`, as any other page; the
console has no scheduler-specific code.

### Mapping from 0012

| 0012 | A scheduler (Nomad, Kubernetes) |
|---|---|
| Job template, parameters | A parameterized job (Nomad `parameterized` + `dispatch -meta`; a Kubernetes Job template filled by its client) |
| `runner: docker` / `host` | The docker driver / a host driver (Nomad `raw_exec`; a node outside the cluster) |
| `notBefore` | A start-time check before the work, or the scheduler's own (Kubernetes `suspend`) |
| `after` | Ordered tasks in one job, or a check that waits on the other job |
| `when: after-close` | A calendar check before the work |
| `when: background-may-run` | A health check before the work (0011) |
| `QUEUE_MAX_RUNNING` | The node's schedulable resources, which the jobs' limits fill |
| The journal, logs | The scheduler's job history and log retention |
| The Queue widget, `tt queue` | The scheduler's UI and command line |

## Consequences

- tinytrader no longer holds Docker write access, and the console no longer
  proxies a scheduler. Less code to keep, and no state format of its own.
- A deployment must bring a scheduler. The examples run none: a quickstart
  needs no batch work.
- The yield and after-close rules are now a deployment's few lines of shell,
  against stable platform contracts (observe `/health`, the calendar), rather
  than a platform feature. A different scheduler reimplements them, briefly.
- The console loses its Queue view (waiting, running and finished jobs, run
  now, edit, cancel, logs). The scheduler's own UI replaces it, outside the
  console's write gate: who may run jobs is that scheduler's to decide.
