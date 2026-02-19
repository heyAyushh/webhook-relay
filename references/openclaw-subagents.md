# OpenClaw Sub-Agents (GitHub & Linear)

OpenClaw agents can use **sub-agents**: dedicated agents for a specific source (e.g. GitHub, Linear) that proactively watch and interact with the relay. A sub-agent can either be an existing agent or one that is spawned on demand when not already present.

## Why Sub-Agents

- **Proactive watching** — A GitHub or Linear sub-agent can monitor the relay (incoming webhook flow), poll APIs, or react to events without the parent `coder` agent having to handle every source.
- **Source-specific behavior** — Sub-agents can hold source-specific SOUL/TOOLS (e.g. “GitHub reviewer” vs “Linear triager”) while the parent coordinates or delegates.
- **Spawn-or-use** — If an actual agent process is not running for that source, OpenClaw can spawn it; otherwise the relay can route to an existing agent. Configuration determines whether to use an existing agent or spawn one.

## Roles

| Sub-agent   | Role relative to relay |
|------------|-------------------------|
| **GitHub** | Proactively watch PR/review events flowing through the relay; react (review, comment); optionally invoke or hand off to parent `coder` when needed. |
| **Linear** | Proactively watch issue/comment events from the relay; triage, update status, add comments; optionally invoke or hand off to parent `coder` for deeper work. |

Neither is “the relay” itself — the relay remains adnanh/webhook + relay scripts. Sub-agents are consumers of the same pipeline: they are invoked by OpenClaw when events arrive (via existing hook mappings) or when the parent agent delegates to them. They can also be spawned if no agent instance is available.

## Configuration Options

1. **Use existing agents**  
   Map `source=github-pr` and `source=linear` to dedicated agent IDs (e.g. `github-coder`, `linear-coder`) in `hooks.mappings`. Those agents must already be defined in `~/.openclaw/agents/`. The relay keeps sending to the same OpenClaw gateway; only the `agentId` in the mapping changes.

2. **Spawn when not present**  
   If OpenClaw supports spawn-on-invoke, configure mappings so that when an event arrives for a source and no instance is running, OpenClaw spawns the appropriate sub-agent. Otherwise, ensure the sub-agent process is running (e.g. via HEARTBEAT or a process manager) so it is “present” when events arrive.

3. **Parent delegates**  
   The main `coder` agent’s AGENTS.md can list the GitHub and Linear sub-agents. On webhook events, the gateway can still invoke `coder` first; `coder` then delegates to the sub-agent (or the mapping can invoke the sub-agent directly).

## Relationship to the Relay

```
GitHub/Linear → adnanh/webhook → relay script → sanitize → OpenClaw Gateway
                                                                  │
                                                        hooks.mappings
                                                                  │
                                    ┌─────────────────────────────┼─────────────────────────────┐
                                    │                             │                             │
                             source=github-pr               source=linear                  (optional)
                             agentId: github-coder          agentId: linear-coder          agentId: coder
                                    │                             │                             │
                                    ▼                             ▼                             ▼
                             GitHub sub-agent              Linear sub-agent              Parent (coordinator)
                             - watch relay flow            - watch relay flow            - delegates to sub-agents
                             - review/comment              - triage/update               - or handles directly
                             - spawn if not present        - spawn if not present
```

- **Relay** = webhook server + relay scripts + sanitization; unchanged.
- **Sub-agents** = OpenClaw agents (existing or spawned) that consume relay output and proactively watch/interact; they do not replace the relay.

## Summary

- OpenClaw supports **sub-agents** for GitHub and Linear.
- Sub-agents **proactively watch and interact** with the relay (they react to events that the relay delivers to OpenClaw).
- You can **use an existing agent** for each source or **spawn** one when not present, depending on OpenClaw configuration.
- This document is the skill reference for sub-agents; it is linked from [openclaw-agents.md](openclaw-agents.md) and [SKILL.md](../SKILL.md).

See [openclaw-agents.md](openclaw-agents.md) for agent config (SOUL.md, AGENTS.md, TOOLS.md) and [openclaw-relay.md](openclaw-relay.md) for relay and hook mappings.
