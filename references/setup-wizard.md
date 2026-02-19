# First-Time Setup Wizard

When the user wants to set up the webhook relay for the first time, walk them through this interactive questionnaire. Use AskUserQuestion for each phase. Collect answers before generating config files.

## Phase 1: Scope

Ask:
- **Which sources?** GitHub only, Linear only, or both?
- **GitHub account type?** Personal account or organization?
- **How many repos?** Specific repos or all repos on the account/org?

## Phase 2: Infrastructure

Ask:
- **Where will the webhook server run?** Local machine, VPS, Docker, or existing server?
- **Is OpenClaw colocated?** Same machine as webhook server (localhost) or separate host?
- **Tailscale?** Already using Tailscale, want to set it up, or prefer a different public ingress (e.g. ngrok, Cloudflare Tunnel)?
- **Deployment target directory?** Where to write config files (default: `./hooks/`)

## Phase 3: GitHub Setup (if selected)

Ask:
- **GitHub App name?** e.g. `openclaw-coder` (will become `name[bot]` identity)
- **GitHub App permissions** — confirm defaults or customize:
  - Pull requests: Read & write
  - Contents: Read-only
  - Metadata: Read-only
- **Do you already have the App created?** If yes, collect App ID, Installation ID, and private key path. If no, guide them through creation.

## Phase 4: Linear Setup (if selected)

Ask:
- **Linear agent user created?** If yes, collect the user's API key and user ID. If no, guide them through creating the service account.
- **Which Linear team(s)?** Team key(s) the agent should monitor.
- **Linear webhook already configured?** If yes, collect the webhook secret. If no, guide them through setup.

## Phase 5: OpenClaw

Ask:
- **OpenClaw already running?** If yes, collect gateway URL and hooks token. If no, note that OpenClaw setup is a prerequisite.
- **Agent ID** — confirm default or customize:
  - Default: `coder` (single agent handles both GitHub and Linear)

## Phase 6: Secrets

Ask the user to generate or provide each secret. Do NOT generate secrets — let the user provide them or generate with commands:

```bash
# Generate webhook secrets
openssl rand -hex 32  # GITHUB_WEBHOOK_SECRET
openssl rand -hex 32  # LINEAR_WEBHOOK_SECRET
openssl rand -hex 32  # OPENCLAW_HOOKS_TOKEN
```

Collect:
- `GITHUB_WEBHOOK_SECRET` (new or existing)
- `LINEAR_WEBHOOK_SECRET` (new or existing)
- `OPENCLAW_HOOKS_TOKEN` (new or existing)
- `GITHUB_APP_ID` (from GitHub App settings)
- `GITHUB_APP_PRIVATE_KEY` path (from GitHub App)
- `GITHUB_INSTALLATION_ID` (from GitHub App installation)
- `LINEAR_AGENT_API_KEY` (from Linear agent user)
- `LINEAR_AGENT_USER_ID` (from Linear agent user)

## Phase 7: Generate

After collecting all answers, generate these files in the target directory:

### Files to generate

1. **`hooks.yaml`** — adnanh/webhook hook definitions (only include hooks for selected sources)
2. **`scripts/relay-github.sh`** — GitHub relay script (if GitHub selected)
3. **`scripts/relay-linear.sh`** — Linear relay script (if Linear selected)
4. **`scripts/sanitize-payload.py`** — copy from skill's bundled script
5. **`.env`** — all secrets and config (remind user: add to .gitignore)
6. **`openclaw-hooks.yaml`** — OpenClaw hooks.mappings config snippet
7. **`transforms/github-pr.ts`** — OpenClaw transform (if GitHub selected)
8. **`transforms/linear.ts`** — OpenClaw transform (if Linear selected)

### After generation

Print a checklist of remaining manual steps:

```
Setup checklist:
[ ] Install webhook: brew install webhook
[ ] Create GitHub App at Settings > Developer settings > GitHub Apps
    - Set webhook URL to: https://{tailscale-hostname}/hooks/github-pr
    - Set webhook secret to value in .env
    - Install on your account/org
[ ] Create Linear webhook at Settings > API > Webhooks
    - Set URL to: https://{tailscale-hostname}/hooks/linear
    - Set secret to value in .env
[ ] Copy .env values to your deployment
[ ] Copy openclaw-hooks.yaml into your OpenClaw config
[ ] Copy transforms/ into ~/.openclaw/hooks/transforms/
[ ] Start: webhook -hooks hooks.yaml -verbose -port 9000
[ ] Expose via Tailscale: tailscale funnel --bg 9000
[ ] Test with: gh webhook forward (see github-hooks.md)
```

## Question Flow Rules

- Ask one phase at a time, not all at once
- Skip phases for unselected sources (e.g. skip Phase 4 if Linear not selected)
- Use sensible defaults — only ask when the user's choice matters
- If the user says "defaults" or "just set it up", use:
  - Both sources (GitHub + Linear)
  - Colocated OpenClaw (localhost:3000)
  - Tailscale Funnel for ingress
  - Default agent ID (`coder`)
  - Target directory: `./hooks/`
