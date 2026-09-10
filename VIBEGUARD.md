# VIBEGUARD.md

This project uses VibeGuard before AI-generated code changes.

## Operating Rule

The agent should inspect, auto-fix low-risk safety issues, and only ask the user
when a step can destroy data, spend money, deploy externally, or requires private
credentials.

## Deployment Confirmation Rule

For every real external production deployment, and any deployment whose target
is unknown, immediately before execution state the exact target and action and
wait for fresh user confirmation. Never infer, reuse, or bypass approval from
earlier wording such as "deploy it" or "handle it yourself".

## Update Cadence Rule

Do not refresh VibeGuard on every task. Run the audit first. If the audit says
the local guardrails are stale, run
`npx --yes @taehwandev/vibeguard@latest update .` once and rerun the audit.
The default refresh interval is 7 days and can be tuned in `.vibeguard.json`.

## Chat Secret Rule

If the user pastes a secret in chat, treat it as exposed. Do not repeat it, pass
it as a command argument, write it to logs, save it to repository files, add it
to GitHub secrets yourself, or deploy it to a server. Tell the user to rotate
the value and enter a new one only through a local provider UI or a secret-store
prompt.

## Audience

The default mode is for non-developers. Show distinct status emoji, keep the
summary simple, and handle safe fixes behind the scenes. Developers can adjust
`.vibeguard.json` for stricter thresholds and project-specific rules.

## Gates

| Gate | Block When |
| --- | --- |
| Security | Secret values, private keys, unsafe auth, missing env ignore |
| Cost | Paid APIs, recurring infrastructure, model calls, or cloud services without simpler alternatives, quotas, or test/prod separation |
| Data | DB deletion, migration, production data, destructive scripts |
| Structure | Oversized files, unclear ownership, risky one-shot edits |
| Repository | Wrong remote, public/unknown visibility, sensitive Git changes |

## Local Shared Rules

/Users/taehwan/git/tao-agent-os

## Cost-Aware Architecture Rule

Do not solve small product requests by stacking paid services, new databases,
queues, background workers, model calls, analytics SDKs, or cloud resources by
default. Prefer existing code, static/local behavior, server-side reuse,
caching, batching, rate limits, and explicit budget boundaries. Ask before
adding anything that can create recurring cost or operational burden.

## Web Architecture Reuse Rule

For web projects, do not duplicate client-side fetching, model calls, paid API
calls, or provider SDK setup in many components. Route repeated work through
shared server-side helpers or endpoints, cache stable or slow-changing data on
the server, batch repeated requests, and add rate limits before exposing a path
that can create cost or quota pressure.

## Required Workflow

1. Run `vibeguard audit .`.
2. Run `vibeguard audit . --fix` for fixable safety issues.
3. Do not print secret values.
4. Rotate any secret that may already have been exposed.
5. Before commit or push, confirm `git remote -v`, repository visibility, and changed files.
6. Show changed files and verification evidence before finishing.
