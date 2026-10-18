# Launcher ↔ game contract v0

Status: **in force** (2026-09-26). The launcher (`rerac-launcher`, a separate Tauri repo) and this game repo both
implement exactly the text in "Contract (verbatim)". The section after it, "Clarifications", records how the game
side implements the points the contract leaves open. Nothing in it changes a field, a command or a code; where it adds
something, the addition is optional for the launcher to use.

Background and the decisions behind it: `docs/plan/launcher_extractor.md`, `docs/plan/decisions.md`.

## Contract (verbatim)

## Launcher ↔ game contract v0 (the orchestrator's spec; the launcher and the game repo both implement exactly this)
- A **version** (one downloadable game build, official or a mod) is a folder `versions/<source>/<version>/` containing `rerac-manifest.json`:
  `{"schema":1,"name":"rerac","version":"0.1.0","game":"rac1","runtime":"<relative path to game binary>","extractor":"<relative path to extractor binary>","supported_discs":["SCUS_971.99"],"data_format":1}`
  The launcher's Development source points at a local folder with this manifest (e.g. a dev build folder).
- **Extractor CLI** (`rerac-extract`):
  - `rerac-extract identify --iso <path> --json` → identifies the disc only.
  - `rerac-extract extract --iso <path> --out <game_data_dir> [--ntsc-only] --json` → full extraction.
  - `rerac-extract verify --out <game_data_dir> --json` → re-hashes against the built-in size/SHA-1 table.
  - With `--json`, stdout is JSON lines, one object per line:
    - `{"type":"progress","stage":"identify|copy|verify","done":<bytes>,"total":<bytes>,"file":"<path>"}`
    - `{"type":"info","message":"..."}`
    - `{"type":"disc","serial":"SCUS_971.99","region":"NTSC-U","version":"1.00","supported":true}`
    - `{"type":"error","code":<n>,"message":"..."}`
    - `{"type":"done","elapsed_ms":<n>}`
  - The process exit code equals the error code.
  - Codes:
    - 0 ok
    - 10 cannot open/read file
    - 11 not an ISO9660 image
    - 20 not Ratchet & Clank (unknown disc)
    - 21 Ratchet & Clank but an unsupported build/region (only SCUS_971.99 v1.00 for now)
    - 30 write failure
    - 31 not enough disk space
    - 40 verification failed
    - 99 internal error
  - On success the extractor writes `<game_data_dir>/extract-info.json`: `{"disc":"SCUS_971.99","data_format":1,"extractor_version":"...","ntsc_only":false,"files":<n>,"bytes":<n>}`.
- **Runtime:**
  - `<runtime> --version-json` prints `{"name":"rerac","version":"...","game":"rac1","data_format":1}` and exits.
  - The launcher starts the game with `<runtime> --data-dir <game_data_dir>`; the env var `RC_DATA_DIR` is equivalent.
- **Folders:** per-OS app data root named `rerac`, e.g. macOS `~/Library/Application Support/rerac/`. Under it:
  - `versions/`
  - `games/rac1/data/` (the extracted game data)
  - `logs/`
  - `settings/`

  The user can move the data root. User-facing text says **ReRAC** (technical identifier `rerac`); no other project name appears anywhere user-facing.

## Clarifications (game side, `crates/rc-extract`)

### Output stream
1. **One terminal line.** Every run ends with exactly one `done` line (exit code 0) or exactly one `error` line (exit
   code = its `code`). Nothing follows it. The exit code is authoritative if the stream is cut short (process killed).
2. **Extra fields are additive.** Objects may carry fields beyond the contract; the launcher must ignore unknown
   fields. Today only the `disc` line has extras:
   `{"type":"disc","serial":"SCUS_971.99","region":"NTSC-U","version":"1.00","supported":true,"game":"rac1","title":"Ratchet & Clank","elf_sha1":"72fd…"}`.
   `game` is `rac1`, `rac2`, `rac3`, `racdl` or `unknown`.
3. **When the `disc` line appears.** Whenever the boot ELF named by `SYSTEM.CNF` was read and hashed: on success (0),
   for code 21, and for code 20 when the disc is a PS2 disc of another game (`"game":"unknown"`,
   `"supported":false`). It comes after the `identify` progress lines and before any `error` line. There is no `disc`
   line for codes 10 and 11, or for code 20 without a readable boot ELF.
4. **Progress.** `done`/`total` are bytes; `total` is fixed for a stage. `file` is the archive-relative path (forward
   slashes) of the file most recently started; for `identify` it is the boot ELF's disc path (`SCUS_971.99`). Lines are
   rate-limited to about 10 per second, and each stage always ends with a line where `done == total`.
5. **Stages per command.** The contract's `stage` values are `identify|copy|verify`; the game side adds a fourth,
   **`prepare`** (the Tier 1 engine cache, 2026-09-26). A launcher shows it as its own step ("Prepare game data").
   - `identify`: `identify` (the boot ELF is hashed).
   - `extract`: `identify`, then `copy`, then `prepare`. The SHA-1 of every file is computed while it is copied and
     checked against the built-in table in the same pass, so `extract` has no separate `verify` stage. A mismatch
     fails with code 40. `prepare` runs automatically at the end (clarification 17).
   - `verify`: `verify` (every file is re-read from disk and hashed).
   - `prepare`: `prepare`. Its `done`/`total` are bytes of the Tier 0 sources processed; `file` is the source's
     archive path (`levels/NN/core_data.bin`).
   - `export`: `export` (clarification 18). `done`/`total` are Tier 0 bytes, weighted per job; `file` is the main
     Tier 0 source of the job most recently started (`levels/NN/core_data.bin`, `global/sound_bank.bin`, …).
6. **Info lines** are human-readable and not meant to be parsed, with one exception kept stable for bug reports: for an
   unknown R&C build (code 21) one info line starts with `new build DB row: ` followed by a ready-to-paste Rust row for
   `crates/rc-extract/src/build_db.rs`.

### Codes in practice
| Code | Raised when |
|---|---|
| 10 | The image cannot be opened or read (missing, permission, I/O error), **or it is truncated**: it has fewer sectors than its ISO 9660 primary volume descriptor declares, or a lump runs past its end. |
| 11 | No ISO 9660 primary volume descriptor at sector 16 (random files, CHD, CSO, too small), **or a raw 2352-byte-sector image** (`.bin`); only 2048-byte `.iso` images are accepted for now. |
| 20 | An ISO 9660 image that is not a Ratchet & Clank disc (no `SYSTEM.CNF`, or a boot ELF of another game). |
| 21 | A Ratchet & Clank disc that is not SCUS_971.99 v1.00: other RC1 regions and demos (SCES_509.16, SCPS_150.37, …), an RC1 serial whose boot-ELF SHA-1 is unknown, and the sequels (R&C 2, 3, Deadlocked; `game` says which). |
| 30 | Creating the output folder or writing, renaming or deleting a file under `--out` failed. |
| 31 | The free space at `--out` is below the bytes to be written plus 64 MiB (checked before anything is written), or a write failed with "disk full". |
| 40 | `extract`: a copied file's size or SHA-1 differs from the table (the disc image is damaged). `verify`: a file is missing, has the wrong size or SHA-1, or `extract-info.json` is missing or unreadable. Info lines name up to 50 bad files. |
| 99 | A bug, or invalid command-line arguments (the message starts with `usage:`). |

### Extraction behaviour
7. **Cancelling = killing the process.** The extractor is crash-safe instead of catching signals:
   - `extract-info.json` is deleted when `extract` starts and written after the last archive file (temp file +
     rename), so **a data folder without `extract-info.json` is incomplete** and must not be launched. The `prepare`
     stage runs after it (the cache stamp hashes it); killing `extract` during `prepare` leaves a complete archive
     and a partial cache, which the game or the next `prepare` completes.
   - Each file is written as `<name>.partial` and renamed only after all its bytes are written and its SHA-1 matched.
     A killed run leaves at most a few `.partial` files and never a complete-looking wrong file.
   - Re-running `extract` into the same folder removes stale `.partial` files and rewrites everything. The launcher may
     also simply delete the folder.
8. **`--ntsc-only`** skips the PAL copies, which the NTSC-U game never reads: every `levels/NN/gameplay_pal.bin`,
   every `levels/NN/scene/KK_pal.bin`, `global/credits_images_pal/*` and the PAL FMVs `global/mpegs/` 021–039,
   052–062, 075–079, 084–087. `extract-info.json` records `"ntsc_only":true` and `verify` then skips those files.
   Files already present from an earlier full extraction are left alone.
9. **`extract-info.json`** has exactly the contract's keys. `files`/`bytes` count the archive files written by this run
   (not `extract-info.json` itself). `extractor_version` is the `rerac-extract` crate version.
10. **Paths.** `--out` is created if missing. `--iso` may be omitted when the environment variable `RC_ISO` is set
    (developer convenience; the launcher always passes `--iso`). `--flag=value` is accepted as well as `--flag value`.
11. **Verify** reads `extract-info.json` to learn the disc and `ntsc_only`, then checks exactly the table's files.
    Extra files in the folder (logs, caches, mods) are ignored.
12. **Extra commands and flags** (not used by the launcher): `table` (developer tool that regenerates the committed
    size/SHA-1 table from a disc), `--threads <n>` (copy/hash/prepare workers, default 4), `--help`, `--version`.
    `prepare` (clarification 17) is optional for the launcher (its "Rebuild cache" action), and so is `export`
    (clarification 18, its "Export assets…" action).

### Runtime and folders
13. The extractor and the runtime never compute the per-OS data root; they only receive paths (`--out`,
    `--data-dir`/`RC_DATA_DIR`). The launcher owns the root and the `versions/`, `logs/`, `settings/` folders.
14. **Runtime exit codes.** When the runtime cannot start, it prints one `error: ...` line on stderr and exits before
    any window opens (`crates/rc-engine/src/disc_source.rs`). The launcher should show that line with the message:

    | Exit | Raised when | Launcher |
    |---|---|---|
    | 0 | Normal quit (also after `--version-json`) | |
    | 2 | `--data-dir` given without a value | "ReRAC couldn't start." (a launcher/game mismatch) |
    | 3 | The data folder is missing, is not a data folder (no `toc.bin`), or the extraction is incomplete (no `extract-info.json`) | "The game data is missing or incomplete." + Re-extract |
    | 4 | `extract-info.json` has another `data_format`, or cannot be read | "The game data doesn't match this ReRAC version." + Re-extract |

    Any other non-zero exit (a panic is 101; a signal has no code) is a crash; point at the log. Unknown arguments are
    ignored with a warning, so a newer launcher's extra flags do not stop an older runtime. `--data-dir` wins over
    `RC_DATA_DIR`, and `--data-dir=<dir>` is accepted.
15. **Packaged version folder** (`tools/package/package.sh`, `docs/workflows/release.md`): `rerac` (`rerac.exe` on
    Windows), `rerac-extract`, `assets/shaders/*.wgsl`, `rerac-manifest.json`, `README.txt`. The manifest names
    the binaries by these relative paths. The runtime finds `assets/` next to its real (symlink-resolved) executable,
    so the folder can be copied or moved as a whole, but `rerac` must not be copied out of it alone.
    macOS packages also hold `rerac-metal` (crate rc-metal: the same game on Metal without Bevy, shaders embedded,
    same CLI, exit codes and `--version-json`; `docs/plan/metal_renderer.md`), named by the optional manifest key
    `"runtime_metal":"rerac-metal"` (additive: same path rules as `runtime`; launchers that do not know it ignore it).
    The launcher starts it instead of `runtime` when its renderer setting is `metal` and the file exists.
16. `<game_data_dir>` (`games/rac1/data/`) holds the Tier 0 archive directly, in the same layout as the development
    `extracted/` tree: `boot/`, `toc.bin`, `global/`, `levels/NN/`, plus `extract-info.json`. See
    `docs/plan/launcher_extractor.md` §4.1. `extract` adds `cache/v1/` (clarification 17); `verify` ignores it.

### Engine cache (Tier 1)
17. **`rerac-extract prepare --out <game_data_dir> [--json]`** builds `<game_data_dir>/cache/v1/` from the Tier 0
    archive in that folder. It never reads the disc image, so it is how the launcher rebuilds the cache after a game
    update ("Rebuild cache"). Output: `progress` lines with `"stage":"prepare"`, `info` lines, one `done` or `error`
    line, as for every command.
    - **Content (v1):** every level's `core_data`, `gameplay_ntsc` and HUD banks, WAD-decompressed (133 lumps,
      about 440 MiB for the full disc), plus `stamp.toml` (cache version, converter version per kind, hash of
      `extract-info.json`). Layout and file format: `docs/plan/launcher_extractor.md` "Tier 1 as built".
    - **Incremental:** lumps that are valid and built from the same source bytes are kept; a stale cache (other
      cache or converter version, other extraction) is emptied and rebuilt; `cache/v<N>` folders of other versions
      are removed. About 0.2 s either way on the dev machine (release build, 4 workers).
    - **Codes:** 10 (no `toc.bin`, or a Tier 0 file cannot be read), 30/31 (writing the cache failed / disk full;
      the free-space check assumes 3 bytes per source byte not yet cached), 40 (a Tier 0 lump is not a valid WAD
      stream: the archive is damaged; run `verify`), 99.
    - **Inside `extract`**, a `prepare` failure does not fail the extraction: the archive is complete and the game
      builds any missing lump itself on first use. One `info` line says so, and `extract` still ends with `done`
      (exit 0). Cancelling still cancels.
    - **The runtime** reads the cache through `rc-data` and never requires it: a missing, stale or damaged cache is
      rebuilt lazily (one log line per lump), and an unwritable folder falls back to in-memory decompression. The
      runtime exit codes (clarification 14) are unchanged.

### Exports (Tier 2)
18. **`rerac-extract export --out <game_data_dir> [--to <dir>] [--what <kinds>] [--level NN] [--json]`** writes
    the optional "usable formats" from the Tier 0 archive in `<game_data_dir>` (never the disc image, never the
    Tier 1 cache). Output: `progress` lines with `"stage":"export"`, `info` lines, one `done` or `error` line, as
    for every command. The game never reads the exports.
    - **`--to`** defaults to `<game_data_dir>/exports/` (outside `cache/`; `verify` ignores it like any extra
      folder). It is created if missing; files already there are overwritten, others are left alone.
    - **`--what`**: a comma list of `textures`, `audio`, `models`, `levels`, `collision`, `text`, or `all`
      (the default). `levels` and `models` also write the level textures their materials reference.
    - **`--level NN`** exports that level only (and no global data); without it every level and the global data
      (global sound bank, music and voice folders, `all_text`, the global HUD) are exported.
    - **Layout, formats and sidecars:** `docs/plan/launcher_extractor.md` "Tier 2 as built". The folder's
      `export-info.json` is removed when an export starts and written last (kinds, levels, file and byte counts
      per folder, skipped items); files are written as `*.partial` and renamed, and a re-run removes leftover
      `*.partial` files. So cancelling is killing the process, as for `extract`.
    - **Items a loader rejects** (none on SCUS_971.99) are skipped with an `info` line starting `skipped: ` and
      listed in `export-info.json`; the export still ends with `done`.
    - **Codes:** 10 (no `toc.bin`, `--level` not in the archive, or a Tier 0 file cannot be read), 30/31 (writing
      failed / disk full; the free-space check estimates the output from the source sizes plus 64 MiB), 40 (a
      level's core or gameplay lump does not parse: the archive is damaged; run `verify`), 99 (bad arguments:
      the message starts with `usage:`).
    - **Size and time** (full disc, dev machine, 4 workers): about 60,000 files and 3.5 GiB (audio 2.2 GiB as
      16-bit WAV) in about 10 s; `--level 01` about 3,300 files and 170 MiB in under a second.
