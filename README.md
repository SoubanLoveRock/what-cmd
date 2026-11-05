# whatcmd

Find terminal commands by describing them in plain English.

[中文](README.zh-CN.md)

![whatcmd cover](docs/cover.png)

![whatcmd demo](docs/demo.gif)

Type part of a command, press `Ctrl-G`, describe what you want, and pick a candidate. whatcmd inserts it at the cursor. It never runs anything for you.

- **Anchored search.** Whatever is already on the command line narrows the scope. With `git ` typed, only git subcommands and options are searched.
- **Command tree index.** Commands, subcommands (`git remote add`) and long options (`git clone --branch`) are indexed as separate nodes.
- **Semantic matching.** Your query is embedded and compared against generated descriptions, so `undo my last commit` can find `git reset` without keyword overlap.
- **Learns from use.** Picked candidates get a small, decaying boost.
- **Safe by design.** Only the command line buffer is edited. Examples that touch `rm`, `dd`, `mkfs` and similar are marked and need confirmation.

## Install

Requirements: macOS or Linux, zsh, [fzf](https://github.com/junegunn/fzf), and an API key for an OpenAI-compatible embeddings endpoint. Claude Code is only needed to run `reindex` yourself.

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

## How it works

### Query

Each `Ctrl-G` runs `whatcmd query --prefix "$LBUFFER" -- <text>`. The steps are:

1. **Anchor (local only).** The text left of the cursor is split on whitespace. An unfinished last word (no trailing space) is dropped. The first token must be a known root command; otherwise the query exits `2` with a spelling hint when the edit distance is 2 or less. Later tokens that name a subcommand move the anchor down the tree. Tokens starting with `-` are skipped, a standalone `--` stops anchoring, and the first other token stops it. So `git -C repo clone ` anchors at `git`.
2. **Embed.** The normalized query text is sent to `embedding.endpoint`. An empty query exits `2`; an embedding failure exits `3`.
3. **Score inside the anchor subtree.** A node's self score is the cosine similarity between the query vector and the node vector. Vectors are L2-normalized, so this is a dot product. A node also inherits `0.8^depth` times the score of any descendant, so `git clone` can rank highly because `--branch` matches. Only nodes scoring at least `0.30` are candidates.
4. **Fall back.** If nothing in the anchor subtree qualifies, the search moves to the parent, then up to the root. Candidates found above the anchor are labeled with `via` so the widget can show that they sit outside the current scope.
5. **Rank.** An exact match between the query text and a node key is placed first. Everything else is sorted by score plus a usage bonus of `min(0.10, 0.05 × ln(1 + count))`, which halves every 30 days. Long options already present on the line are not suggested again.

Inserting a candidate writes only the part of its path after the anchor, at the cursor, and replaces an unfinished word if there is one. `RBUFFER` is untouched and Enter is never sent. The scoring constants are fixed in code for now. They will move to config only after they have been tuned on a fixed sample set.

### Index

Files live in `index_dir` (default `~/.local/share/whatcmd`):

- `index.json` holds the schema version (currently 2), the generator metadata (agent version and prompt version), the embedding model and dimensions, the SHA-256 of `vectors.bin`, and one entry per node. Command nodes are full paths such as `git remote add`. Parameter nodes are long options such as `git clone --branch`. Each entry records its parent, description, examples, source hash, vector row and `pinned` flag.
- `vectors.bin` is one little-endian `f32` row per node, with a row length equal to the configured dimensions.

Both files are written to a `.tmp` name and renamed into place. On load, whatcmd rejects a schema mismatch, a `vectors.bin` whose size does not match the node count, and a checksum mismatch. The checksum catches a crash between the two renames, which could otherwise leave a new `index.json` next to an old `vectors.bin`.

### Seed index

The binary embeds a built-in seed of about 120 POSIX commands with their descriptions and vectors (`BAAI/bge-m3`, 1024 dimensions). When `index_dir` has no `index.json`, the seed is written there. No LLM call is made. If the configured model or dimensions differ from the seed, the seed is skipped and the first reindex builds the index instead. Seed nodes are pinned: reindex neither regenerates nor removes them.

### Reindex

`whatcmd reindex` runs in this order:

1. Takes `index.lock` so two runs cannot write at the same time.
2. Scans every executable on `$PATH` and removes duplicate names.
3. For each command, reads the man page and falls back to `--help`. Output is stripped of ANSI codes and backspaces, then truncated to `max_doc_bytes` (64 KiB by default).
4. Sends each command's documentation to [Claude Code](https://claude.com/claude-code) as `claude -p --tools "" --model haiku --output-format json --json-schema …`. Up to `agent.concurrency` batches run at once, each with a 600-second timeout. The prompt is versioned and is part of the source hash, so a prompt change regenerates the affected nodes. Unchanged docs under the same prompt version are reused.
5. Subcommands named by the model are checked with `<cmd> <sub> --help` before they are queued. The check accepts output on stdout, tries `-h` for nested paths, and falls back to the parent's help text. The walk stops at `reindex.max_depth` (3 by default).
6. Each result is validated and written to a staging file under `~/.cache/whatcmd/staging`. Embedding runs afterwards, in batches. If embedding fails, the staged results remain and the next run embeds them without calling the LLM again.
7. The new index replaces the old one atomically. A node that fails keeps its previous entry. The exit code is `0` if everything succeeded and `1` if anything failed.

Progress and warnings go to stderr. The same lines are appended to `reindex.log` in the cache directory.

### Safety

- The model gets no tools. `--tools ""` is what disables them; in local experiments, `--allowedTools` alone still let Bash run. The sample was small, so treat this as evidence rather than proof.
- The model's output is parsed as JSON and validated before anything is stored. Command names must match `^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$`. Options must start with `--`. Descriptions are limited to 400 characters, and there are at most three examples of 200 characters each.
- The insert path only edits the command line. Examples containing `rm`, `dd`, `mkfs`, `> /dev/`, `chmod -R` or `chown -R` are marked, and selecting one asks for `y` before insertion.
- The API key is read only from the environment variable named in `api_key_env`. A config file that contains `api_key` is rejected.
- Usage records store only the picked key and a timestamp.

### Known limitations

- Ranking is uneven for short or vague queries. Prompt and description quality are the main factors, and tuning is ongoing.
- Each query needs the embedding endpoint. There is no offline fallback yet.
- Reindex needs a logged-in `claude` CLI. Queries need only the embedding key.
- Not implemented yet: `whatcmd doctor`, the automatic background reindex (`reindex --auto`), and a `-v` / `WHATCMD_LOG` switch for logging.
- Supported shells and platforms: zsh only, on macOS and Linux.

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
