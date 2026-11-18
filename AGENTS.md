# Working on contr-core

contr-core is contr's core: the helper `herdr-contr-feed`, the
orchestration tool `herdr-contr-orch`, the hc client `herdr-core` and the
Claude Code kit. The contr-desktop app and the agents use them.

## Where the code comes from

- The first commit is a copy of Flockbook (MIT) at one commit; `NOTICE` names
  it. There is no upstream remote and no plan to merge from it (owner,
  2026-11-11). Change, move or delete any file.
- Keep `LICENSE` and the copyright lines.
- contr's core came from herdr-contr with its history (D44, D45, 2026-11-17).
  An issue or PR number up to 269 in a comment, a decision or a commit
  message is herdr-contr's.
- The orchestration (worker profiles, spawn, the owner's questions and
  to-dos, the MCP tools, the kit) moves here from the contr repository
  (#160): `xplat/herdr-contr-orch` and `kit/`. The helper's `install` puts both in
  the Herdr home (`herdr-contr/kit`, `herdr-contr/bin`). The tool's files
  (`agents.json`, `owner.json`, `state/`) are in the core's root:
  `CONTR_HOME`, else the Herdr home's `herdr-contr` folder (#255). The
  default root is the neutral folder `~/.config/contr-core` when it exists,
  else the old neutral folder `~/.config/contr-backend` when it exists, else
  `~/.config/herdr-app/herdr-contr` when it exists (#264; decision
  `3-core-home`). The
  mod reads and changes them only through the tool (#186).

## Layout

| Path | Side | Holds |
|---|---|---|
| `install.sh` | core | the release zip's installer: it runs the helper's `install` |
| `docs/protocol.md` | core | the contract of the helper, which contr-desktop uses (JSON Lines) |
| `xplat/herdr-contr-feed` | core | the helper (Rust): reads the agents' session logs and talks to hc; its `install` puts the core in its root |
| `xplat/herdr-core` | core | the helper's hc client, its parsers and `places` |
| `xplat/herdr-contr-orch` | core | the orchestration tool (Rust), ported from contr slice by slice (#160); its parts are in the list below the table |
| `kit`, `.claude-plugin/marketplace.json` | core | the Claude Code plugin, with agent definitions, the playbook, the chat-writing skill, hooks and the MCP server. Codex coordinators use the same playbook and MCP server through `herdr-contr-orch codex coordinator` |
| `xplat/fixtures` | core | the session logs and hub answers that the helper's tests read |
| `xplat/build-stamp.rs` | core | the build stamp of the helper and the tool |
| `xplat/herdr-contr-feed/web` | core | the phone page that the helper embeds (built output; do not edit) |
| `web`, `scripts/web-build.mjs`, `scripts/sw-test.mjs` | core | the phone page's source, its build into `xplat/herdr-contr-feed/web`, and its service worker test |
| `docs/design`, `docs/research` | core | the notes on the phone page's protocol and on the question keys, that the helper's code names |
| `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `scripts/land.sh` | core | the CI, the release and the landing of a PR: one job and one zip |

This repository has no `herdr-app/`. The default build reads no file of it.
The CI job `backend` checks it.

The parts of `xplat/herdr-contr-orch`:

- the profiles and the flow
- the owner file and its open questions and to-dos as JSON (`owner open --json`)
- the profiles file as JSON and its changes (`agents config --json`, `agents set`)
- the worker commands on hc (`spawn`, `resume`, `ls`, `title`, `send`, `interrupt`, `stop`, `rm`, `read`, `prompt`)
- the MCP server with the coordinator's tools and a worker's `report` tool (`mcp`)
- the reports held for a busy coordinator (`held`)
- the limits of spawn and send: workers at once and open PRs (`limit`)
- the worker and coordinator hooks for Claude Code and Codex (`herdr-contr-orch hook --help`)
- the check of a PR against the brief's boundary (`pr check-boundary`)

## Checks

- `cargo test --locked --manifest-path xplat/Cargo.toml -p herdr-contr-feed -p herdr-core -p herdr-contr-orch`.
- `node --test scripts/release-notes.test.mjs` when you change the release notes script.
- It must pass, and the `CI` check on the PR, before you report done.

## Design rules (lessons from contr)

- **Data comes from hooks, session logs and APIs, not from the screen.** A
  new feature takes its data from an agent hook, the session log, hc's
  API or the orchestration tool. Terminal text is only for the guard that
  checks the prompt before a key goes out.
- **One inbox.** All that needs the owner is in one list, with one policy
  for each kind of action (`auto` or `ask`). Do not add a second channel for
  the same thing. An answer in any place closes the item.
- **State comes from the source.** Show what GitHub, hc and the
  orchestration tool say now. Do not keep a state file that someone writes by hand.
- **No file that each PR appends to.** A decision is one file:
  `docs/decisions/<PR number>-<slug>.md`. Do not keep a table that each PR
  edits. Register each test fixture in a file of its own.
- **The life of a PR is code.** Worktree, CI wait, merge and cleanup are
  commands, not steps that a model does by hand.
  `scripts/land.sh <pr> [--after <pr>] [--shot <file>]` lands a PR: update,
  CI wait, squash merge of the exact head, install of merged `main`.
## Releases

- `.github/workflows/release.yml` runs after the `CI` check passes on a push
  to `main`. It builds the helper for macOS (universal) and Linux (musl,
  x86_64 and aarch64) and the orchestration tool for macOS. It publishes
  one zip as the release `build-<commit date>-<short sha>`, and deletes all
  but the last 10 `build-` releases (with their assets, `release.json` too).
- The version is `0.1.<run number>` of the workflow. Each build job sets it
  in `HERDR_CONTR_VERSION`, so `herdr-contr-feed version` says
  `"release":"0.1.<run>"`; the "Check the stamp" steps check it.
- The zip holds `install.sh`, `bin/`, `remote/<target>/` and
  `remote/SHA256SUMS`. On the other machine: unzip, then `./install.sh`.
- `scripts/release-notes.mjs` writes the notes and the asset `release.json`
  (the same approach as contr-desktop's). The notes start with
  `contr-core 0.1.<run> from <sha>.`, then a `## Changes` part: one `- `
  line for each commit subject after the release before, newest first (no
  release before: the last 20; no commits: no part), then the build and
  install lines. `release.json` (schema 1) has `version`, `tag`, `sha`,
  `date` (ISO time), `assets` (the zip), `sha256` (the zip's hash) and
  `releases`: the newest 10 releases with a version line, this one first,
  each with `version`, `tag`, `date` (`YYYY-MM-DD`) and `changes`.
  contr-desktop reads it. CI runs the script's test
  (`node --test scripts/release-notes.test.mjs`) and a dry run.
- The build stamp (`<short sha> <commit date>`) comes from
  `HERDR_CONTR_BUILD_SHA` and `HERDR_CONTR_BUILD_DATE`, else from git
  (`xplat/build-stamp.rs`).
- To sign the macOS binaries, set the secrets `MACOS_CERT_P12` (a
  Developer ID Application certificate as base64 `.p12`),
  `MACOS_CERT_PASSWORD` and `MACOS_SIGN_IDENTITY`. Without them the
  release is unsigned and its notes say so.
- Run the workflow by hand (`workflow_dispatch`) to test it on a branch.

## Safety

- The owner's Herdr app and hc core are live. Do not edit files in the
  owner's Herdr home (`~/.config/herdr-app`) or in the core's neutral root
  (`~/.config/contr-core`, or the old `~/.config/contr-backend`) by hand.
  Never send input to the owner's Herdr app. Never stop, restart or send
  input to the owner's hc core.
- No agent runs `herdr-contr-feed home move`, with or without `--dry-run`,
  on the owner's files. The owner runs it (#264). Tests run it in a
  temporary HOME.
- Never start `herdr-app` (owner, 2026-11-12). It is one app per Mac: a
  second start, even with a test home and a scratch `HC_SESSION`, replaces
  the owner's window. The coordinator looks at merged `main` in the
  owner's app (see below).
- An agent can install merged `main` into the real Herdr home with the
  helper's `install` (owner, 2026-11-12): build the helper and the
  orchestration tool, run `install --dry-run` first, then `install --helper
  <file> --orch <file>`. After it, `herdr-app do
  layout.switch` and `herdr-app shot` are allowed, to look at the result.
  Do not install a branch that is not merged.
  On a machine with no Herdr app, `install --backend` is the form: it
  writes only the kit and the programs into the core's root.
- Tests use fakes and fixtures. No test reads the owner's session logs
  (`~/.claude`, `~/.codex` and the others), and no test starts the real
  `claude` program.
- Do not turn on remote access (the phone page) on the owner's Mac.
- On the owner's Mac, run one heavy job at a time: a Herdr app run or a
  release build.

## Testing

- Add a test for each new behavior, error paths included.
- Fixtures look like real Claude Code sessions.
## Workflow

- Work is tracked in GitHub issues and pull requests. One issue, one branch
  `agent/<issue>-<slug>`, one PR into `main`, ideally under about 400
  changed lines. A PR that only deletes can be larger.
- Update a branch by merging `main` into it. Never force-push.
- Squash merge. The coordinator merges after CI passes on the exact head,
  without the owner's approval (owner, 2026-11-12). A builder does not merge
  its own PR.
- Docs, comments and commit messages: concise ASD-STE100 Simplified
  Technical English. No `Co-Authored-By`, AI attribution or generated-by
  lines.
- Write no local paths (such as `/Users/...`) in the repository or in commit
  messages. Do not @-mention people. Name other GitHub users or repositories
  only in `NOTICE`, `THIRD_PARTY_NOTICES.md`, `CONTRIBUTORS.md` and
  `LICENSE`.
- Temporary files go in `tmp/`, which git ignores.
- When a change makes a doc wrong (`docs/protocol.md`, `README.md`,
  this file), update it in the same PR.
- Report the PR number, the head SHA, the check status and any deviation.
