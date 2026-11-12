# babeltower

Single-binary language learning entry for the terminal. Words are looked up in
an offline SQLite dictionary; sentences go through DeepSeek. The wordbook is
one CSV file, writable offline and mergeable with `aitr merge`. There is no
daemon, web UI, database server, or network requirement for word lookup.

## Install

```bash
curl -LsSf https://raw.githubusercontent.com/mixsoulgithub/babeltower/main/install.sh | sh
```

The `curl` installer needs no sudo. It downloads the release archive for the
current platform, verifies the sha256 file, installs `aitr` into `~/.local/bin`
(override with `--dir`), and accepts an explicit `--version`:

```bash
curl -LsSf https://raw.githubusercontent.com/mixsoulgithub/babeltower/main/install.sh | sh -s -- --version 0.1.0
curl -LsSf https://raw.githubusercontent.com/mixsoulgithub/babeltower/main/install.sh | sh -s -- --dir "$HOME/bin"
```

Release assets are `babeltower-<target>.tar.gz` plus
`babeltower-<target>.sha256`; the archive contains the `aitr` binary. Pushing a
`v*` tag runs `.github/workflows/release.yml` and builds Linux x86_64 and macOS
x86_64/arm64 assets.

### From source

```bash
cargo build --release
./bin/aitr
```

`bin/aitr` is a shell shim for `target/release/aitr`. To install a local build:

```bash
ln -sfn ~/projects/babeltower/bin/aitr ~/.local/bin/aitr
```

After either install, fetch and build the offline dictionary once:

```bash
aitr dict fetch
aitr dict build
```

## Data

Path: `~/.local/share/words/words.csv` (`AITR_CSV` overrides). UTF-8 CSV with
a fixed header:

```
uuid,word,translation,phonetic,tags,hit,last,box,due,reviews,reviewed,updated,origin,deleted
```

- `uuid`: stable identity. Migration and new rows use UUIDv5 of
  `babeltower:` + casefolded word, so two machines converge on the same word.
- `updated`: Unix seconds of the last content change, used by merge.
- `origin`: `AITR_ORIGIN`, or first 12 hex chars of sha256(`/etc/machine-id`).
- `deleted`: tombstone flag. Deletes are never physical; use `aitr gc --force`.
- `box` 1..5, intervals 1/2/4/8/16 days. `good` advances a box and schedules
  from the current box; `again` resets to box 1 and makes the word due today.
- `last`, `due`, `reviewed`: `YYYY-MM-DD` or empty.

Writes take a `<csv>.lock` flock, write a temporary file, fsync, then rename.
The first open migrates the old v1 header
`id,word,translation,hit,last,box,due,reviews,reviewed`; a v1 backup is written
to `<csv>.v1.bak` if it does not exist.

## Dictionary

```
aitr dict fetch                 # ECDICT + CC-CEDICT, refuses overwrite without --force
aitr dict fetch --ecdict
aitr dict fetch --cedict --force
aitr dict build                 # default paths
aitr dict build --ecdict ./ecdict.csv --cedict ./cedict.txt --out ./dict.sqlite
aitr dict info
```

Default files:

- `~/.local/share/babeltower/ecdict.csv` (`AITR_DATA_DIR` overrides the dir)
- `~/.local/share/babeltower/cedict.txt`
- `~/.local/share/babeltower/dict.sqlite` (`AITR_DICT` overrides the file)

Sources and licenses:

- ECDICT, https://github.com/skywind3000/ECDICT, MIT. The CSV is downloaded
  from `raw.githubusercontent.com/skywind3000/ECDICT/master/ecdict.csv`.
- CC-CEDICT, https://www.mdbg.net/chinese/dictionary?page=cc-cedict,
  CC BY-SA 4.0. The gzip is downloaded from
  `mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz`.

`aitr dict fetch` and sentence translation use reqwest, which honors
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
`DEEPSEEK_API_KEY`, then `~/.config/aitr/deepseek` (`AITR_CONFIG_DIR` overrides
the directory). Request: `deepseek-chat`, max_tokens 1024, temperature 0,
30 second timeout. Results are cached in-process (256 entries, FIFO). Word
lookup never touches the network.

## CLI

```
aitr                        # TUI
aitr box                    # dictionary or AI, prints translation
aitr -f box                 # source + translation
aitr -r box                 # query and save
aitr -l                     # wordbook CSV: hit,word,translation,last
echo text | aitr            # stdin
aitr --ai some text         # force AI
aitr merge other.csv [--dry-run] [--report report.csv]
aitr import old-v1.csv
aitr gc [--days 90] [--force]
aitr dict fetch|build|info
aitr --version
```

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
- Enter query or open detail, `s` save, `:ai` force AI, `Esc` clear
- word results show every sense and `Enter detail (N more)` when folded;
  the detail layer scrolls with `j/k`

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
`AITR_THEME=mono` keeps selection, errors, and modes distinguishable without
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
