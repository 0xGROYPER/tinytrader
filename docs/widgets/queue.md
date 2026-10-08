# Queue

Work that runs later, or when the machine can spare it: research collectors, notebooks, builds. Each job comes from one of the deployment's job templates (decision 0012), with its parameters, and waits until its conditions hold: a start time, after the market's close, while no live capture is running (decision 0011), or after other jobs. Lists what waits and runs, then what finished, newest first.

**Data:** the queue service, through the console. Docker jobs run as one-off containers; host jobs (builds) run on the Mac through its host runner at background priority.

**Using it:**

- **Add a job:** pick a template, fill in its parameters (each must match the pattern shown), choose when it may run, and optionally a start time.
- **Waiting jobs:** Run now (drops its conditions) or Edit (start time, condition).
- **Running jobs:** Cancel stops them; Log shows their output as it comes.
- From a terminal, `tt queue list`, `tt queue add TEMPLATE NAME=VALUE --when after-close`, `tt queue cancel ID`.
