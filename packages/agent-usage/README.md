# yggdrasil-agent-usage

A bar add-on for Yggdrasil: how much of Claude's limits is used, in the top bar ("Claude 27% · 2d 12h", red when the week's use is
ahead of an even pace), with the weekly, 5-hour and model-scoped windows in its card. Adapted from Omarchy's agent-usage plugin
(robzolkos/omarchy-agent-usage, MIT).

The numbers come from Anthropic's usage address, asked with Claude Code's own sign-in (`~/.claude/.credentials.json`): the token
goes only to api.anthropic.com, through curl's standard input (not on a command line), and is never stored or refreshed here.
Answers are cached for four minutes. No Claude Code sign-in: no pill.

    yggdrasil-agent-usage          readable
    yggdrasil-agent-usage --bar    the JSON the bar reads (docs/INTEGRATIONS.md, bar-item)

## Agents (a second bar item from the same package)

`agents.conf` adds **Agents**: the number of AI agents running, with a ring for the busiest limit any of them is near. Its card
lists every agent this computer knows of with its state (working, idle, stopped), its usage and reset time where known, and when
it last did something:

- **Claude Code's sessions**, from Claude Code's own notes (`~/.claude/sessions/PID.json`: its status, busy or idle). Only those
  files are read, never the `.key` files beside them, and a session counts as running only while its own process is (the start
  time it noted is checked in `/proc`). The last activity is also the time of its transcript (only the time; nothing in it is
  read). Claude's limits are this add-on's own reading, the same four-minute cache as the pill above.
- **Sarina and other agents' usage records**, from `sarina fleet-status --json` (Sarina's docs/ECOSYSTEM.md, section 1). That
  text is another program's and is cleaned again here (control, bidi and zero-width characters removed, cut short). A Sarina
  without `--json` leaves those rows out and says so under the card.

It is run when something it reads changes (the folders are watched: Claude Code's notes, the usage records, this add-on's cache,
Sarina's sessions; at most once a minute), every ten minutes otherwise, and every half minute while its card is open. Hide it (or
the usage pill) in Settings > Appearance > Top bar and dock.

    yggdrasil-agent-usage --agents     the JSON the bar reads for Agents
    yggdrasil-agent-usage --act howto  what the card's one button does: opens this README

### How to start one

The Agents item only shows; it never starts, stops or signals an agent. To start one I use a terminal:

    sarina fleet-launch claude "the task"      # or codex, gemini, ...; prints the plan and asks before it runs

`fleet-launch` shows exactly what it would run and starts it only after a `y` (`--yes` skips only that question); it has no
auto-approve mode, and the agent it starts still asks before acting. It is a command for a person at a terminal on purpose: no button, no model and no bar item can
run it. Every attempt is logged (`~/.local/state/sarina/agent-launch.jsonl`, the prompt only as a hash and a short preview).
Claude Code itself starts as usual with `claude` in a project folder.
