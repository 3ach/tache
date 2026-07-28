# tache

Task dependencies (a DAG) layered over Todoist, which has none natively.

Edges are stored inside Todoist itself as `after:` lines in a task's
description (`after: buy lumber` — resolved by id, exact name, or unique
substring within the same project). A webhook-driven server recomputes the
unblocked frontier on every change and maintains two labels:

- `next` — every prerequisite is complete; actionable now (includes
  dep-free tasks)
- `blocked` — waiting on an active prerequisite, an ambiguous reference,
  or a cycle

The "custom view" is then a plain Todoist filter, which works in every
official app:

```
(overdue | today) | @next
```

There is no polling. Todoist webhooks trigger reconciles; `tache sync` is
the manual fallback if a delivery is ever dropped.

## CLI

```
tache serve       # webhook server (deployment mode)
tache sync        # one-shot reconcile (run once at setup / on demand)
tache frontier    # list actionable tasks
tache graph       # print dependency edges
tache doctor      # unresolved / ambiguous refs, cycles
tache dep "stain shelves" "buy lumber"        # add edge
tache dep "stain shelves" "buy lumber" --rm   # remove edge
```

Config via env / `.env` — see `.env.example`.

## Deployment

Push to `main` → GitHub Actions runs tests, builds the image to
`ghcr.io/3ach/tache`, and runs `deploy/droplet/deploy-main.sh` on
zach.network as `zach` (docker group, no root). The container binds
`127.0.0.1:8321`; Caddy fronts it at `tache.zach.network` via a one-time
snippet in `/etc/caddy/conf.d/tache.caddy`. Runtime secrets live in
`/home/zach/tache/.env` on the droplet, never in the repo or CI.

The Todoist side needs a one-time app at https://developer.todoist.com/
with webhook URL `https://tache.zach.network/todoist-hook`, OAuth redirect
URL `https://tache.zach.network/oauth/callback`, and events `item:added`,
`item:updated`, `item:completed`, `item:uncompleted`, `item:deleted`. Its
client id/secret go in the droplet `.env` (the secret also verifies the
`X-Todoist-Hmac-SHA256` signature on webhooks).

Todoist only activates webhooks for a user after an OAuth handshake, so
tache hosts it: visit `/oauth/start`, approve, and the server exchanges
the code, persists the token to `/data` (a volume, survives redeploys),
swaps it in live, and runs a first sync. No manual token handling.
