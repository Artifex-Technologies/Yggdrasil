# yggdrasil-agent-usage

A bar add-on for Yggdrasil: how much of Claude's limits is used, in the top bar ("Claude 27% · 2d 12h", red when the week's use is
ahead of an even pace), with the weekly, 5-hour and model-scoped windows in its card. Adapted from Omarchy's agent-usage plugin
(robzolkos/omarchy-agent-usage, MIT).

The numbers come from Anthropic's usage address, asked with Claude Code's own sign-in (`~/.claude/.credentials.json`): the token
goes only to api.anthropic.com, through curl's standard input (not on a command line), and is never stored or refreshed here.
Answers are cached for four minutes. No Claude Code sign-in: no pill.

    yggdrasil-agent-usage          readable
    yggdrasil-agent-usage --bar    the JSON the bar reads (docs/INTEGRATIONS.md, bar-item)
