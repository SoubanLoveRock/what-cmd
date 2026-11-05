# whatcmd

Find terminal commands by describing them in plain English.

![whatcmd demo](docs/demo.gif)

Type part of a command, press `Ctrl-G`, describe what you want, and pick a candidate. whatcmd inserts it at the cursor. It never runs anything for you.

- **Anchored search.** Whatever is already on the command line narrows the scope. With `git ` typed, only git subcommands and options are searched.
- **Command tree index.** Commands, subcommands (`git remote add`) and long options (`git clone --branch`) are indexed as separate nodes.
- **Semantic matching.** Your query is embedded and compared against generated descriptions, so `undo my last commit` can find `git reset` without keyword overlap.
- **Learns from use.** Picked candidates get a small, decaying boost.
- **Safe by design.** Only the command line buffer is edited. Examples that touch `rm`, `dd`, `mkfs` and similar are marked and need confirmation.

## Install

Requirements: macOS or Linux, zsh, [fzf](https://github.com/junegunn/fzf), and an API key for an OpenAI-compatible embeddings endpoint.

Download the archive for your platform from [Releases](https://github.com/SoubanLoveRock/what-cmd/releases), then:

```bash
tar -xzf whatcmd-0.1.0-<target>.tar.gz
cd whatcmd-0.1.0-<target>
install -m 755 whatcmd ~/.local/bin/          # any directory on PATH
mkdir -p ~/.config/whatcmd ~/.local/share/whatcmd/shell
cp config.example.toml ~/.config/whatcmd/config.toml
cp shell/whatcmd.zsh ~/.local/share/whatcmd/shell/
```

On macOS, a downloaded binary may be quarantined. Clear it with `xattr -d com.apple.quarantine ~/.local/bin/whatcmd`.

Or build from source (Rust 1.85+):

```bash
cargo install --path . --locked
```

## Setup

1. Put your embedding key in the environment. It is read only from the variable named in `api_key_env` and is never written to config:

   ```bash
   export WHATCMD_EMBEDDING_API_KEY=...   # e.g. in ~/.zshenv or from your keychain
   ```

2. Load the widget in `~/.zshrc`:

   ```bash
   source ~/.local/share/whatcmd/shell/whatcmd.zsh
   ```

3. Open a new shell and press `Ctrl-G`.

The release ships a seed index of about 120 POSIX commands built with `BAAI/bge-m3` (1024 dimensions), so it works right away if you keep the example config.

## Usage

| In the input bar | Effect |
|---|---|
| plain text | Search, then pick with arrows and Enter |
| `/reindex` | Open the menu and rebuild the index |
| `Esc` | Close and leave the command line unchanged |

From the CLI:

```bash
whatcmd query --prefix "git " -- push my commits to remote
whatcmd record "git push"
whatcmd reindex --names git,docker   # index specific commands
whatcmd reindex                      # scan all of PATH
```

Exit codes: `0` results, `1` no results (or some reindex failures), `2` config, index or anchor error, `3` embedding service unavailable.

## Building the index

`reindex` scans `$PATH`, reads each command's `man` page or `--help`, and asks [Claude Code](https://claude.com/claude-code) (`claude -p`, with all tools disabled and a JSON schema) to write descriptions. Descriptions are then embedded and written atomically to `index_dir`.

It needs the `claude` CLI on `PATH` and is incremental: unchanged docs are skipped.

## Privacy

- Query text and generated descriptions are sent to your embedding endpoint.
- Command documentation is sent to Claude during `reindex`.
- Usage records (`usage_file`) store only the picked key and a timestamp, locally.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
scripts/package.sh            # dist/whatcmd-<version>-<host>.tar.gz
```

Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds macOS and Linux archives (arm64 and x86_64) and attaches them to a GitHub release.

Design notes are in [docs/DESIGN.md](docs/DESIGN.md), interfaces in [docs/INTERFACES.md](docs/INTERFACES.md), and the decision log in [DEVLOG.md](DEVLOG.md). Those documents are in Chinese.
