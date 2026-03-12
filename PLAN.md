# OAC Slack Bot — Implementation Plan

## Context

This bot needs to:
- Respond to Slack messages (mentions + DMs)
- Detect intent from the message and route to the right **OAC skill** (= claude plugin)
- Execute that skill with full plugin capabilities
- Support plugins internally calling other plugins (chaining)
- Maintain thread context for follow-up questions

---

## Key Insight: Use Claude Code SDK, Not Raw Anthropic API

The claude-plugins (skills) were designed to run inside **Claude Code CLI** — they have SKILL.md
as a system prompt and expect Claude's native tools: `Task`, `Bash`, `Read`, `Glob`, `Grep`, plus
any MCP tools (Coralogix, K8s, GitHub, Watchtower, etc.).

If we use the raw Anthropic API, we'd need to re-implement all those tools. Instead:

> Use `@anthropic-ai/claude-code` SDK — the same runtime the Claude CLI uses.
> Skills work out of the box. Plugin-to-plugin chaining via the `Task` tool is native.

This is exactly what `~/experimental/claude-code-slack-bot-v2` already does — it wraps
the Claude Code SDK with Slack. We build on that pattern.

---

## Option Comparison

### Option A — Stateless Plugin Router (Nexus-style)

```
Slack message
  → detect plugin from message
  → load SKILL.md as system prompt
  → run single Claude Code SDK call
  → post response
  → forget everything
```

**Pros:**
- Simple. Fast to build (~2 days).
- No database, no session management.
- Each call is isolated — no state leakage between users.

**Cons:**
- No memory. Follow-up questions ("why did that fail?") lose all context.
- Plugin chaining works within one call, but the result isn't carried forward.
- Can't do multi-turn OAC debugging workflows ("check logs → now check deployments → compare").

**Best for:** Read-only FAQ bots, single-shot answers.

---

### Option B — Stateful Plugin Bot with Thread Sessions (Autobot-style)

```
Slack message
  → thread_ts → look up or create Claude session for this thread
  → detect intent → inject plugin's SKILL.md as system prompt
  → run Claude Code SDK (full tools: Task, Bash, MCP servers)
  → stream response back to Slack
  → session stays alive for follow-up messages
  → cleanup after inactivity
```

**Pros:**
- Thread = conversation. Follow-up questions have full context.
- Plugin chaining is native: plugin A calls `Task` → plugin B runs → result flows back.
- MCP servers (Coralogix, K8s, GitHub, Watchtower) work out of the box.
- Skills work exactly as they do in Claude CLI — no translation layer.
- Multiple parallel incidents don't pollute each other (thread isolation).

**Cons:**
- More complexity (~5 days to build properly).
- Sessions need cleanup (in-memory with TTL is sufficient for Phase 0).

**Best for:** OAC incident triage, multi-turn debugging, knowledge queries.

---

## Recommendation: Option B

The core OAC use case is multi-turn: an engineer @mentions the bot, gets a diagnostic report,
then asks follow-up questions ("what changed in that deployment?", "show me the logs again").
Stateless doesn't support this.

Plugin chaining (rzp-discover calling coralogix calling k8s-debugger) needs the Claude Code SDK
runtime anyway — Option B gives us that for free.

**Phase 0**: Build Option B but with in-memory sessions (no Redis/Postgres needed).
**Phase 1**: Plug in Redis for session persistence if needed.

---

## Architecture

```
Slack (Socket Mode)
    │
    │  @mention or DM
    ▼
SlackHandler (Bolt)
    │
    │  channel + thread_ts → session key
    ▼
SessionManager
    │
    ├── existing session? → resume
    └── new session? → create
              │
              │  detect intent from message
              ▼
         PluginRouter
              │
              ├── known plugin keyword → load SKILL.md → inject as systemPrompt
              └── no match → run with base system prompt (general assistant)
                        │
                        ▼
              Claude Code SDK (query())
                        │
                        ├── Built-in tools: Task, Bash, Read, Glob, Grep
                        ├── MCP servers: Coralogix, K8s, GitHub, Watchtower, etc.
                        └── Plugin chaining: Task tool → spawn sub-agent with another SKILL.md
                        │
                        ▼
              StreamHandler → posts to Slack thread
```

---

## Plugin Execution Model

### How a skill runs

1. User: `@oac how does emandate-service handle NPCI timeouts?`
2. Router detects: recurring-payments domain → `rzp-discover:recurring-payments` skill
3. Load `~/.agents/skills/rzp-merchant-post-onboarding-skill/SKILL.md` → set as `systemPrompt`
4. Claude Code SDK runs with that system prompt + all native tools
5. If the skill calls `Task(subagent_type="coralogix")` → coralogix plugin runs as a sub-agent
6. Results flow back up the chain → final response posted to Slack

### Plugin-to-plugin chaining (native)

The `Task` tool in Claude Code SDK lets a plugin spawn any other agent/plugin as a sub-process.
This is exactly how `rzp-discover:brainstorm` routes to `rzp-discover:cards-nb-and-wallets`
which routes to `rzp-discover:coralogix`. No custom invoke_plugin needed — it's built in.

### Plugin registry

Scan `~/.agents/skills/*/SKILL.md` and `~/claude-plugins/plugins/*/skills/*/SKILL.md`.
Parse frontmatter for `name` and `description`. Build a map used for routing.

---

## Tech Stack

| Component | Choice | Why |
|-----------|--------|-----|
| Language | TypeScript | Consistent with existing bots |
| Slack SDK | `@slack/bolt` (Socket Mode) | No public URL needed |
| Claude | `@anthropic-ai/claude-code` SDK | Skills work natively |
| Sessions | In-memory Map with TTL | No infra needed for Phase 0 |
| Plugin registry | File scan at startup | Zero config |
| Logging | `winston` | Structured JSON logs |

---

## Project Structure

```
oac-slack-bot/
├── src/
│   ├── index.ts              # Entry point
│   ├── config.ts             # Env vars (Zod-validated)
│   ├── slack/
│   │   ├── app.ts            # Bolt app (Socket Mode)
│   │   ├── events.ts         # @mention + DM handlers
│   │   └── streamer.ts       # Stream Claude output → Slack messages
│   ├── claude/
│   │   ├── runner.ts         # Wraps Claude Code SDK query()
│   │   └── sessions.ts       # Thread → session mapping + TTL cleanup
│   ├── plugins/
│   │   ├── registry.ts       # Scan SKILL.md files → build registry
│   │   └── router.ts         # Intent detection → pick plugin
│   └── logger.ts
├── .env.example
├── package.json
└── tsconfig.json
```

---

## Implementation Plan

### Day 1 — Skeleton + Slack Connection

**Goal:** Bot responds to @mentions with a static reply.

Tasks:
- [ ] Init TypeScript project (copy tsconfig, package.json pattern from claude-code-slack-bot-v2)
- [ ] Install: `@slack/bolt`, `@anthropic-ai/claude-code`, `winston`, `zod`
- [ ] `config.ts` — validate env vars: `SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN`,
      `SLACK_SIGNING_SECRET`, `ANTHROPIC_API_KEY`
- [ ] `slack/app.ts` — Bolt app in Socket Mode
- [ ] `slack/events.ts` — handle `app_mention` and `message` (DMs)
- [ ] Static reply to verify connectivity

**Test:** @mention bot in Slack → see "I'm alive" reply.

---

### Day 2 — Plugin Registry

**Goal:** Load all 115 plugins from disk at startup.

Tasks:
- [ ] `plugins/registry.ts`:
  - Scan `~/.agents/skills/*/SKILL.md`
  - Scan `~/claude-plugins/plugins/*/skills/*/SKILL.md`
  - Parse YAML frontmatter (name, description)
  - Skip `_` prefixed skills (internal)
  - Return `Map<name, { name, description, systemPrompt, path }>`
- [ ] Log count of loaded plugins at startup
- [ ] `plugins/router.ts`:
  - Keyword matching against plugin names + descriptions
  - Fallback: return null (use base system prompt)

**Test:** Add a `/plugins` slash command that lists all loaded plugins.

---

### Day 3 — Claude Code SDK Integration

**Goal:** Messages are processed by Claude with the right plugin's SKILL.md injected.

Tasks:
- [ ] `claude/sessions.ts`:
  - `Map<string, Session>` where key = `${channel}-${threadTs || channel}`
  - Each session has: `abortController`, `lastActivity`, `pluginName`
  - Background cleanup: destroy sessions idle > 30 min
- [ ] `claude/runner.ts`:
  - Wraps `query()` from `@anthropic-ai/claude-code`
  - Takes: `userMessage`, `sessionKey`, `systemPrompt?`, `workingDirectory`
  - Streams `AssistantMessage` tokens back via async generator
  - Session resume: pass `sessionId` if session exists
- [ ] Wire it up in `slack/events.ts`:
  - On mention: detect plugin → load SKILL.md → call runner → stream to thread

**Test:** @mention "what is nexus?" → bot routes to `nexus-platform-faq` skill → answers correctly.

---

### Day 4 — Streaming + Slack Message Management

**Goal:** Long responses update a single Slack message (no spam).

Tasks:
- [ ] `slack/streamer.ts`:
  - Post initial "Thinking..." message → get `message_ts`
  - Buffer tokens, update message every 500ms (Slack rate limit)
  - On completion: final update, add ✅ reaction
  - On error: update with error message, add ❌ reaction
- [ ] Handle Slack's 3000 char limit: split into thread replies if needed
- [ ] Add typing indicator (emoji reaction while processing)

**Test:** Ask a long question → see single message updating in real time.

---

### Day 5 — MCP Servers + Plugin Chaining

**Goal:** Plugins can use MCP tools (Coralogix, K8s, GitHub, etc.) and call other plugins via Task.

Tasks:
- [ ] Add `mcp-servers.json` config file (copy from `claude-code-slack-bot-v2` pattern)
  - Add Coralogix MCP (already configured in your env)
  - Add K8s / Friday MCP
  - Add GitHub MCP
  - Add Slack MCP (for viveka retrieval)
- [ ] Pass `mcpServers` config to `query()` call in runner.ts
- [ ] Test plugin chaining: trigger `rzp-discover:brainstorm` → verify it spawns sub-agents
- [ ] `mcp-servers.example.json` for documentation

**Test:** @mention "check the nexus pods in prod" → bot calls k8s-debugger skill → which uses K8s MCP → returns pod status.

---

### Day 6 — OAC Skill Integration

**Goal:** Full OAC triage workflow works end-to-end.

Tasks:
- [ ] Load OAC SKILL.md from `~/claude-plugins/` or define inline
- [ ] Add routing rules for OAC trigger patterns:
  - `[FIRING]`, `[CRITICAL]`, `triage`, `investigate`, `diagnose`
  - Explicit: `@bot oac <alert>`
- [ ] Add `oncall-debugger` skill routing
- [ ] Add `systematic-solver-v2` skill routing
- [ ] Test with a real alert pattern

**Test:** `@bot [FIRING] HighDBConnectionTimeout in emandate-service` → OAC triage report.

---

### Day 7 — Polish + Deployment

**Goal:** Production-ready bot.

Tasks:
- [ ] `/help` command — list available plugins with descriptions
- [ ] `/plugin <name>` — force-invoke a specific plugin
- [ ] Rate limiting: max 3 concurrent requests per user
- [ ] Graceful shutdown (SIGTERM → stop Bolt → drain in-flight requests)
- [ ] `.env.example` with all required vars
- [ ] `Dockerfile` for containerized deployment
- [ ] `docker-compose.yml` for local dev

---

## Environment Variables

```env
# Slack (Socket Mode)
SLACK_BOT_TOKEN=xoxb-...
SLACK_APP_TOKEN=xapp-...
SLACK_SIGNING_SECRET=...

# Claude
ANTHROPIC_API_KEY=...
CLAUDE_MODEL=claude-sonnet-4-20250514   # optional override

# Bot behavior
PLUGIN_DIRS=~/.agents/skills,~/claude-plugins/plugins  # colon-separated scan paths
SESSION_TTL_MINUTES=30
MAX_CONCURRENT_SESSIONS=50
WORKING_DIRECTORY=~/work                # default cwd for Claude Code SDK

# MCP (configured in mcp-servers.json)
CORALOGIX_API_KEY=...
GITHUB_TOKEN=...
```

---

## Slack App Manifest

```yaml
display_information:
  name: OAC Bot
  description: On-Call Automation Agent — triage incidents, query knowledge, debug services
  background_color: "#1a1a2e"

features:
  bot_user:
    display_name: OAC
    always_online: true
  slash_commands:
    - command: /oac
      description: Invoke OAC directly
      usage_hint: "[alert details or query]"
    - command: /plugins
      description: List available plugins

oauth_config:
  scopes:
    bot:
      - app_mentions:read
      - channels:history
      - chat:write
      - groups:history
      - im:history
      - im:read
      - im:write
      - reactions:write

settings:
  event_subscriptions:
    bot_events:
      - app_mention
      - message.im
  interactivity:
    is_enabled: true
  socket_mode_enabled: true
  token_rotation_enabled: false
```

---

## Plugin Routing Rules

| User says | Routes to |
|-----------|-----------|
| `[FIRING]`, `[CRITICAL]`, `triage` | `oncall-debugger` |
| `investigate`, `diagnose`, `root cause` | `systematic-solver-v2` |
| `what is nexus`, `how does nexus` | `nexus-platform-faq` |
| `aws cost`, `billing` | `aws-cost-analysis` |
| `k8s`, `pod`, `deployment failed` | `k8s-debugger` |
| `ticket`, `devrev` | `ticket-resolution-analyzer` |
| `razorpay jargon`, `what is <term>` | `razorpay-jargon-explainer` |
| `emandate`, `nbplus`, `pg-router` + question | `rzp-discover:brainstorm` |
| no match | base system prompt (general assistant) |

Router checks in order:
1. Explicit `/plugin <name>` — force override
2. Keyword match on plugin `name` (exact)
3. Keyword match on plugin `description` (semantic)
4. LLM disambiguation if multiple match (lightweight call)
5. Default: general assistant mode

---

## Migration Path (Option A → B)

If you want to start even simpler (Option A, stateless) and migrate later:

**Start with:** Single `query()` call per message, no sessions, no streaming.
**Migrate when:** Engineers ask follow-up questions > 40% of interactions, or
context re-injection becomes painful.

**What stays the same:**
- Plugin registry (same files)
- Plugin router (same logic)
- MCP server config (same JSON)
- SKILL.md format (same files)

**What changes:**
- Add `sessions.ts` (~50 lines)
- Pass `sessionId` to `query()` calls
- Add cleanup background task

---

## What We Are NOT Building (Phase 0)

- ❌ Auto-detection of alerts (manual @mention only)
- ❌ PostgreSQL / Redis persistence (in-memory sessions)
- ❌ Automated remediation actions (read-only)
- ❌ PR generation or code changes
- ❌ PagerDuty / DevRev ticket auto-creation

---

## Definition of Done (Phase 0)

- [ ] Bot online in Slack, responds to @mentions and DMs
- [ ] 115 plugins loaded from disk at startup
- [ ] Intent detection routes to correct plugin ≥ 80% of time
- [ ] Plugin chaining works (test with rzp-discover:brainstorm)
- [ ] Thread context preserved across follow-up messages
- [ ] MCP servers connected (at minimum: Coralogix, K8s)
- [ ] Streaming response updates single Slack message
- [ ] Clean shutdown, no message loss
- [ ] Tested with at least 2 real OAC scenarios:
  - Alert triage: `[FIRING] HighDBConnectionTimeout`
  - Knowledge query: `how does emandate-service work?`
