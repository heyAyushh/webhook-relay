# Coder Orchestrator Contract

Session key: `coder:orchestrator`

## Responsibilities

1. Read `memory/coder-tasks.md`.
2. Decide inline handling vs worker spawn.
3. Spawn worker sessions with event context and task board snapshot.
4. Track workers with `subagents(action=list)`.
5. Steer workers with `subagents(action=steer, ...)`.
6. Update task board on completion/failure.
7. Emit final summary to Telegram topic 2.

## Robust Cross-Session Messaging

Use this pattern when sending to sessions outside current subagent tree:

```python
def robust_send(session_key, message, retries=3):
    for attempt in range(retries):
        sessions = sessions_list()
        if session_key not in [s.key for s in sessions]:
            log(f"session missing: {session_key}")
            return False

        result = sessions_send(session_key, message, timeoutSeconds=10)
        if result.ok:
            return True

        sleep(2 ** attempt)  # 1, 2, 4 seconds

    log(f"sessions_send failed: {session_key}")
    return False
```

Rules:

- Prefer `subagents(action=steer)` for orchestrator-spawned workers.
- Use `sessions_send` for sessions not spawned by orchestrator.
- Always verify session liveness before sending.
- Worker completion/failure reports should go through system events.
