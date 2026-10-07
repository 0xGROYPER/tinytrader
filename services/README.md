# services

Long-running daemons that are shared plumbing. Each meets the service contract in `libs/observe` (`tinytrader.observe/1`: `/health`, `/metrics`, `/describe`) and owns no trading decisions.
