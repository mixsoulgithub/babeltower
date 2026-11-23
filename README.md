# babeltower

Single-binary language learning entry for the terminal. Words are looked up in
an offline SQLite dictionary; sentences go through DeepSeek. Every successful
query is written to the wordbook by default (`--no-record` opts out); existing
translations are kept unless `-r` or the TUI `s` key forces an update. The
wordbook is one CSV file, writable offline and mergeable with `bbt merge`. There
is no daemon, web UI, database server, or network requirement for word lookup.

## Install

```bash
curl -LsSf https://raw.githubusercontent.com/mixsoulgithub/babeltower/main/install.sh | sh
```

The `curl` installer needs no sudo. It downloads the release archive for the
current platform, verifies the sha256 file, installs `bbt` into `~/.local/bin`
(override with `--dir`), and accepts an explicit `--version`:

```bash
curl -LsSf https://raw.githubusercontent.com/mixsoulgithub/babeltower/main/install.sh | sh -s -- --version 0.1.0
curl -LsSf https://raw.githubusercontent.com/mixsoulgithub/babeltower/main/install.sh | sh -s -- --dir "$HOME/bin"
```

Release assets are `babeltower-<target>.tar.gz` plus
`babeltower-<target>.sha256`; the archive contains the `bbt` binary. Pushing a
`v*` tag runs `.github/workflows/release.yml` and builds Linux x86_64 (GNU and
musl) plus macOS x86_64/arm64 assets. The curl installer uses the musl build on
Linux, so it does not depend on the host glibc version.

### From source

```bash
cargo build --release
./bin/bbt
```

For a static Linux binary:

```bash
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools
CC_x86_64_unknown_linux_musl=musl-gcc \
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
cargo build --release --target x86_64-unknown-linux-musl
```

The release profile uses LTO, one codegen unit, and `strip = true`. The binary is
still larger than a pure TUI like `graph-tui` because it also contains SQLite,
a TLS HTTP stack, JSON/CSV parsing, and Unicode tables.

`bin/bbt` is a shell shim for `target/release/bbt`. To install a local build:

```bash
ln -sfn ~/projects/babeltower/bin/bbt ~/.local/bin/bbt
```

The first word query installs the dictionary automatically when `dict.sqlite`
is missing: it downloads ECDICT + CC-CEDICT and builds the database. To
pre-warm it, or to rebuild after a failed download, run:

```bash
bbt dict fetch
bbt dict build
```

## Data

Path: `~/.local/share/words/words.csv` (`BBT_CSV` overrides). UTF-8 CSV with
a fixed header:

```
uuid,word,translation,phonetic,tags,hit,last,box,due,reviews,reviewed,updated,origin,deleted
```

- `uuid`: stable identity. Migration and new rows use UUIDv5 of
  `babeltower:` + casefolded word, so two machines converge on the same word.
- `updated`: Unix seconds of the last content change, used by merge.
- `origin`: `BBT_ORIGIN`, or first 12 hex chars of sha256(`/etc/machine-id`).
- `deleted`: tombstone flag. Deletes are never physical; use `bbt gc --force`.
- `box` 1..5, intervals 1/2/4/8/16 days. `good` advances a box and schedules
  from the current box; `again` resets to box 1 and makes the word due today.
- `last`, `due`, `reviewed`: `YYYY-MM-DD` or empty.

Writes take a `<csv>.lock` flock, write a temporary file, fsync, then rename.
The first open migrates the old v1 header
`id,word,translation,hit,last,box,due,reviews,reviewed`; a v1 backup is written
to `<csv>.v1.bak` if it does not exist.

## Dictionary

```
# The first word query runs this automatically when dict.sqlite is missing.
bbt dict fetch                 # ECDICT + CC-CEDICT, refuses overwrite without --force
bbt dict fetch --ecdict
bbt dict fetch --cedict --force
bbt dict build                 # default paths
bbt dict build --ecdict ./ecdict.csv --cedict ./cedict.txt --out ./dict.sqlite
bbt dict info
```

Default files:

- `~/.local/share/babeltower/ecdict.csv` (`BBT_DATA_DIR` overrides the dir)
- `~/.local/share/babeltower/cedict.txt`
- `~/.local/share/babeltower/dict.sqlite` (`BBT_DICT` overrides the file)

Sources and licenses:

- ECDICT, https://github.com/skywind3000/ECDICT, MIT. The CSV is downloaded
  from `raw.githubusercontent.com/skywind3000/ECDICT/master/ecdict.csv`.
- CC-CEDICT, https://www.mdbg.net/chinese/dictionary?page=cc-cedict,
  CC BY-SA 4.0. The gzip is downloaded from
  `mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz`.

`bbt dict fetch` and sentence translation use reqwest, which honors
`HTTPS_PROXY`/`HTTP_PROXY` when the network needs a proxy. The builder streams
both sources into a temporary SQLite file and renames it into place. ECDICT `translation`/`definition` literal `\n` becomes a real
newline; every sense is kept. CC-CEDICT inserts the simplified head and, when
different, an additional traditional head. `PRAGMA journal_mode=OFF` and
`synchronous=OFF` are used only for the temporary build.

Lookup is exact head first; English then tries `s`, `es`, `ed`, `ing`, `ly`
stems (including simple `e`/double-consonant restoration) and marks the result
as stemmed. Chinese lookup matches both simplified and traditional heads. The
UI shows every sense; `box` shows the noun and verb senses together.

## AI

Only sentences (or `--ai`) use DeepSeek. Key resolution:
`DEEPSEEK_API_KEY`, then `~/.config/bbt/deepseek` (`BBT_CONFIG_DIR` overrides
the directory). Request: `deepseek-chat`, max_tokens 1024, temperature 0,
30 second timeout. Results are cached in-process (256 entries, FIFO). Word
lookup never touches the network.

## CLI

```
bbt                        # TUI
bbt box                    # dictionary or AI, print and record
bbt -f box                 # source + translation
bbt -r box                 # force-update the saved translation
bbt --no-record box        # query without touching the wordbook
bbt -l                     # wordbook CSV: hit,word,translation,last
echo text | bbt            # stdin
bbt --ai some text         # force AI
bbt merge other.csv [--dry-run] [--report report.csv]
bbt import old-v1.csv
bbt gc [--days 90] [--force]
bbt dict fetch|build|info
bbt --version
```

A successful word lookup or sentence translation is recorded automatically.
An existing row only gets `hit`/`last` updated; its `translation`, `phonetic`,
and `tags` are preserved unless empty. `-r` forces the translation to the
fresh result. Failed lookups do not write. If the wordbook cannot be written,
the translation still prints and a warning goes to stderr.

Merge is row-level last-writer-wins on `(updated, origin)`. Counters `hit` and
`reviews` take the maximum; `last`/`reviewed` take the larger ISO date; other
fields come from the winner. Equal timestamps with different content are
reported as conflicts. Duplicate active words keep the newer row and tombstone
the other. Repeating the same merge is a no-op. `--dry-run` does not write;
`--report` writes `uuid,reason,local_updated,remote_updated`.

## TUI

Global: `q` quit, `Ctrl-c` quit anywhere, `?` help, `1..4`/`Tab`/`H`/`L`
switch pages, `:` command. The review page uses `1`/`2` for grading.

Lookup:

- `i`/`a` insert, `/` clear and insert, `j/k` select, `Ctrl-d/u` scroll
- input: Left/Right, Home/End, Delete, Alt-Left/Alt-Right by word,
  Alt-Backspace or Ctrl-W deletes the previous word
- Enter query or open detail, `s` force-save the selected entry, `:ai` force
  AI, `Esc` clear
- word results show every sense and `Enter detail (N more)` when folded;
  the detail layer scrolls with `j/k`
- successful word and sentence lookups are recorded automatically; `s`
  overwrites the saved translation with the selected entry

Words:

- `j/k`, `gg/G`, `Ctrl-d/u`, `/` live filter, `n/N`, `s` sort, `r` reload
- Enter detail, `e` edit translation, `dd` delete, `a` add
- adding a word prefills from the dictionary, sentences from AI

Review:

- `Space` reveal, `1` again, `2` good, `t` flash/type, `R` reverse,
  `n` new session, `Esc` close
- type mode: `i` to resume typing, Enter check, Space/Enter continue
- answers accept any sense after stripping `n.`/`vt.`/`[网络]` prefixes and
  splitting on `;`, newline, `/`, comma

Layout switches at 100 columns (nav rail plus Words list/preview), 80 columns
(narrow 5-column rail, no preview), and below 80 columns (no rail). Below
60x16 only `terminal too small` is shown. `NO_COLOR=1` or
`BBT_THEME=mono` keeps selection, errors, and modes distinguishable without
color.

Commands: `:q`, `:help`, `:ai`, `:dict`, `:merge <path>`, `:import <path>`,
`:gc [days]`, `:sort <due|hit|last|word|box>`,
`:tab <lookup|words|review|stats>`, `:version`.

## Tests

```
cargo test
cargo build --release
```

Fixtures cover ECDICT multi-sense rows, CC-CEDICT multi-entry heads, stemming,
v1 migration, merge idempotency, tombstones, and CLI recording.
