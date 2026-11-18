# contr-core

contr's core: the helper program `herdr-contr-feed`, the orchestration tool `herdr-contr-orch`, their
hc client `herdr-core`, and the Claude Code plugin `contr`. The contr-desktop app and the agents use
them. `AGENTS.md` has the rules and the layout of the repository.

| Path | Holds |
|---|---|
| `xplat/herdr-contr-feed` | the helper: reads the agents' session logs and talks to hc |
| `xplat/herdr-core` | the helper's hc client and conversation parsers |
| `xplat/herdr-contr-orch` | the orchestration tool: the profiles and the flow of the workers, the owner file |
| `kit/`, `.claude-plugin/marketplace.json` | the Claude Code plugin `contr` and the [Codex coordinator playbook](kit/README.md#codex-coordinator) |
| `web/` | the source of the phone page that the helper embeds |

The helper's contract (JSON Lines) is in [docs/protocol.md](docs/protocol.md).

## Checks

```sh
cargo test --locked --manifest-path xplat/Cargo.toml -p herdr-contr-feed -p herdr-core -p herdr-contr-orch
```

## Install from a release

Each commit on `main` that passes CI makes a release `build-<commit date>-<short sha>` with one zip.
Download it, unzip it, then run the installer in its folder (`--dry-run` first to see what it does):

```sh
./install.sh --dry-run
./install.sh
```

The core's files (the profiles, the owner file, the kit and the programs) are in its root:
`~/.config/contr-core` by default. A machine with the old folder `~/.config/contr-backend` keeps using it
until `herdr-contr-feed home move` moves it. [docs/protocol.md](docs/protocol.md) has the full order;
[docs/decisions/3-core-home.md](docs/decisions/3-core-home.md) has the decision.

## License

MIT. See [LICENSE](LICENSE), [NOTICE](NOTICE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
