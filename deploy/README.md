# Deploying the public demo

The demo runs as one container: the `dentalclarity` server with the web app embedded, the synthetic practice and the knowledge base. It uses synthetic data only. Do not use this image for clinical mode or real patient records.

Files in this folder:

| File | Purpose |
|---|---|
| `Dockerfile` | Multi-stage build: web app and server, then a Debian slim runtime |
| `entrypoint.sh` | Runs the server as uid 10001, fixing `/data` ownership first when started as root |
| `fly.toml` | Optional Fly.io configuration, not applied yet |

The repository root has a `.dockerignore` that keeps `target/`, `.env`, `.local/`, `data/`, `eval/runs` and `eval/heldout` out of the build context.

## Build and run locally

Docker with BuildKit is required (Docker Desktop has it). Build from the repository root:

```sh
docker build -f deploy/Dockerfile -t dentalclarity-demo .
```

The first build compiles everything and takes several minutes. Later builds reuse the cached crates and compiled dependencies.

Run it with a named volume for the database:

```sh
docker volume create dentalclarity-data
docker run --rm -p 8080:8080 \
  -v dentalclarity-data:/data \
  -e DC_PUBLIC_ORIGIN=http://localhost:8080 \
  -e OPENROUTER_API_KEY \
  dentalclarity-demo
```

`-e OPENROUTER_API_KEY` with no value copies the key from your shell, so it never appears in the command or in shell history. Leave it out to run without a key: the app and the pre-generated cases work, and new generation and narration report that the provider is not configured.

Open http://localhost:8080. Check health with `curl http://localhost:8080/api/health`.

`DC_PUBLIC_ORIGIN` must match the address in the browser exactly (scheme, host and port), because state-changing requests are refused when their `Origin` differs. Use the `localhost` form above, not `127.0.0.1`, unless you set the origin to match.

## Environment variables

| Variable | Set in image | Notes |
|---|---|---|
| `DC_MODE` | `demo` | Keep `demo`. The image is for the public demo only. |
| `DC_BIND` | `0.0.0.0:8080` | The port the platform's proxy forwards to |
| `DC_DATA_DIR` | `/data` | SQLite database and narration cache. Mount a persistent volume here. |
| `DC_FIXTURE` | `/app/fixtures/synthetic/practice.toml` | Synthetic practice |
| `DC_KNOWLEDGE_DIR` | `/app/knowledge` | Knowledge base |
| `DC_PUBLIC_ORIGIN` | not set | Required in production: the public `https://` address. It turns on `Secure` and `__Host-` cookies and is the expected `Origin` on writes. |
| `DC_TRUSTED_PROXY` | not set | IP address of a reverse proxy whose `X-Forwarded-For` is trusted. One fixed address only. See the Fly notes below. |
| `DC_HSTS` | not set | `true` only when the public certificate is trusted by every browser (any platform-managed certificate is) |
| `DC_DEMO_MONTHLY_BUDGET_USD` | not set (code default 25) | Monthly spend cap |
| `OPENROUTER_API_KEY` | never | Secret. Pass at runtime only, through the platform's secret store. |
| `DC_TEXT_MODEL`, `DC_SPEECH_MODEL` | not set | Optional model overrides |
| `RUST_LOG` | `info` | Log filter |

The image contains no secrets and no `.env` file. The server also reads a `.env` in its working directory if one exists; do not add one to the image.

## Behind a TLS proxy

Every host for this demo terminates TLS at its own proxy and forwards plain HTTP to port 8080. Two settings matter:

- `DC_PUBLIC_ORIGIN` must be the public `https://` URL, for example `https://demo.example.org`. Without it the server assumes `http://0.0.0.0:8080`, refuses every write from the real site, and sends cookies without `Secure`.
- The per-network limits (new visitor sessions, generations per day per IP, narration per network) use the client address. By default that is the TCP peer, which behind a proxy is the proxy itself, so all visitors share one address and one set of per-network limits. If the proxy connects from one fixed address, set `DC_TRUSTED_PROXY` to it and the server reads the rightmost `X-Forwarded-For` entry.

## Deploying to Fly.io

Nothing here has been run yet. Install `flyctl` and sign in with `fly auth login` first. Run every command from the repository root.

1. Pick an app name and replace `CHANGE-ME-dentalclarity-demo` in `deploy/fly.toml` (both `app` and `DC_PUBLIC_ORIGIN`). Change `primary_region` if you want one other than Seattle (`sea`).
2. Create the app without deploying: `fly apps create <app-name>`.
3. Create the volume in the same region: `fly volumes create dentalclarity_data --region sea --size 1 --app <app-name>`.
4. Set the secret: `fly secrets set OPENROUTER_API_KEY=... --app <app-name> --stage`. Paste the key into the command yourself; `--stage` stores it without starting a deploy.
5. Deploy: `fly deploy --config deploy/fly.toml --dockerfile deploy/Dockerfile`.
6. Check: `fly status --app <app-name>`, `fly logs --app <app-name>`, and `curl https://<app-name>.fly.dev/api/health`.
7. Keep a single machine: `fly scale count 1 --app <app-name>`. The database is a file on one volume and cannot be shared between machines.

Notes for Fly:

- **Volume ownership.** Fly mounts volumes owned by root, so `fly.toml` builds the `rootfix` target. The container starts as root only long enough to give `/data` to uid 10001, then runs the server as that user with no capabilities. Other hosts that mount writable volumes can use the default target, which never runs as root.
- **Stop and start.** `auto_stop_machines = "stop"` with `min_machines_running = 0` stops the machine when idle and starts it on the next request, so an idle demo costs only the volume. The first visit after a stop waits a few seconds for the machine to boot. Fly stops machines with SIGINT, which the server handles as a graceful shutdown.
- **Client addresses.** Fly's proxy forwards the visitor address in `Fly-Client-IP` and appends it to `X-Forwarded-For`, but it does not connect from one fixed address, so `DC_TRUSTED_PROXY` cannot be set and the server does not read `Fly-Client-IP` today. On Fly, the per-network limits therefore apply to all visitors together. Per-visitor limits and the budget cap still work as designed. Reading `Fly-Client-IP` safely needs a small server change, not yet made.
- **HSTS.** Fly's certificate is publicly trusted, so `fly.toml` sets `DC_HSTS=true`.

## Budget guard and visitor limits

- **Monthly cap.** `DC_DEMO_MONTHLY_BUDGET_USD` (25 in `fly.toml`) caps model spend per UTC calendar month. Before each model call the server reserves the most the call can cost and refuses it if spend plus reservations would pass the cap. At 80% of the cap, new generations stop with `503 budget_reached`; questions and translations of cached cases continue until 100%. Narration may use at most 20% of the cap. Pre-generated cases keep working after the cap is hit.
- **Per visitor.** Each visitor gets a session from `POST /api/visit`. Limits include 3 new generations per hour, 10 variants per hour, 20 questions per hour and 20,000 narrated characters per hour. Visitor presentations are deleted after 24 hours.
- **Per network.** 30 new sessions per minute, 10 generations per day and 60,000 narrated characters per day per client address (IPv6 grouped by /64).
- **Concurrency.** Two generation jobs at a time across the server, one per visitor; others queue.

The full list is in `docs/SECURITY.md` under Demo abuse controls.

**The `/data` volume must persist.** The spend counter lives in the SQLite database in `/data`. If the volume is lost or the container runs without one, the counter starts again at zero and the month's spend is no longer capped by what was already spent. Rate-limit counters are held in memory, so they reset whenever the server restarts, including after each auto-stop. The monthly cap does not reset.

Set a spending limit at OpenRouter as well, so a lost volume cannot lead to more spend than you planned.
