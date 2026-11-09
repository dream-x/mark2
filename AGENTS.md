# mark2

A custom agent harness in Rust: **core library + headless CLI**, tuned for
running **local models** (llama.cpp / ollama / LM Studio on Apple Silicon).
The core is UI-agnostic: it accepts `Op` (submission) and streams `Event` — the CLI is the
first consumer of this channel, the TUI the second; ACP/server will come later. The first
provider is OpenAI-compatible. Plan: `docs/plans/completed/20260824-rust-agent-harness.md`.

## Structure

- `crates/core` (`mark2-core`) — neutral message and event types, the `Provider` trait,
  agent loop, tools (bash/read/grep/write/edit/web_fetch/web_search), JSONL sessions,
  compaction, config.
- `crates/providers` (`mark2-providers`) — provider implementations: `openai_compat`,
  SSE parsing, JSON repair for tool arguments from weak models.
- `crates/cli` (`mark2-cli`, binary `mark2`) — clap, interactive and `-p` print mode,
  Event stream rendering; `src/tui/` — full-screen chat on ratatui (`App` state is a pure
  function of `Event`/keys, rendering is snapshot-tested with `TestBackend`).
- `crates/bench` (`mark2-bench`) — benchmark runner (`bench/STRATEGY.md`): runs the `mark2`
  binary on the tasks in `bench/tasks/`, the verdict comes from the task's `verify.sh`.

Shared dependencies are declared in the root `[workspace.dependencies]`; crates pull them
in via `dep.workspace = true`, no crate-local versions.

## Commands

- Build: `cargo build`
- Run: `cargo run -q -p mark2-cli -- "задача"`; TUI: `cargo run -q -p mark2-cli` with no
  prompt in a terminal (or `--tui`)
- Pre-commit check:
  `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
- Diff review by a local model (optional, needs ollama): `scripts/review.sh`
  (exit 0 — approve, 2 — needs-work, 1 — failure); if you change the wrapper itself, run
  `sh scripts/test-review.sh`.

Tests are mandatory in every task: code and tests are written within the same task.

A significant change comes with a `CHANGELOG.md` entry in the format
"what was done ← why → what it led to (measurably)"; the reasons for harness controls and
their expiry conditions go to `docs/decisions.md`.

## Running against a local model

The default `base_url` is `http://localhost:8080/v1` (llama-server). If the server is
different (or, as happens, port 8080 is taken by an unrelated web server — then HTML
lands in stderr instead of a response), set the base explicitly:

```sh
# ollama: the OpenAI-compatible prefix is /v1, port 11434
cargo run -q -p mark2-cli -- -p --yolo \
  --base-url http://localhost:11434/v1 --model qwen3.5:9b \
  -C /tmp/scratch "Create a file hello.txt with content exactly: mark2 works"
```

`-p` — print mode (the answer to stdout, service output to stderr), `--yolo` — do not ask
for confirmations, `-C` — the agent's working directory. Persistent settings are easier to
keep in `~/.config/mark2/config.toml` (sections `[provider]`, `[agent]`, `[web]`).
A project `.mark2.toml` (searched upward from cwd, not above `$HOME`) can only tighten:
setting `[provider] base_url`/`api_key_env`, `policy = "yolo"`, a non-empty
`post_edit_command`, or a `memory_file` outside its directory there is a startup error.

Network tools: `web_fetch` is always present unless disabled with `[web] fetch = false`
(bench ablation); `web_search` is registered only if SearXNG is configured —
`[web] search_base_url = "http://localhost:8888"` or the `MARK2_SEARXNG_URL` variable
(non-empty — takes precedence over the files); SearXNG's `settings.yml` must have the
`json` format enabled. Without `--yolo` both ask for confirmation, like `bash`, and are
forbidden in `read_only`.

E2E tests against a live server are marked `#[ignore]` and are not run by
default:

```sh
cargo test -p mark2-cli --test e2e_local -- --ignored
```

Where they go comes from the environment: `MARK2_E2E_BASE_URL` (default
`http://localhost:11434/v1`), `MARK2_E2E_MODEL` (default `qwen3.5:9b`),
`MARK2_E2E_TIMEOUT_SECS` (default 300). Observations from real runs —
`docs/notes/e2e-findings-20260824.md`.
