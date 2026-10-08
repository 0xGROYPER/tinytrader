# Terminal

A terminal program in the browser, such as Emacs, embedded in a panel.

**Data:** a GoTTY server named by the topology's `links.terminal`, run beside tinytrader on loopback, for example `gotty --address 127.0.0.1 --port 8090 --permit-write emacs -nw`. A terminal that accepts typing is a shell on that machine: keep it on loopback, or behind a password and TLS.

**Using it:**

- Click into the panel to type. The browser keeps some shortcuts for itself (Ctrl-W, Ctrl-T, Ctrl-N).
- Each panel starts its own session; with `emacs --daemon` and `emacsclient -t` as the command, every panel shares one Emacs.
