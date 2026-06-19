# Plan: copy-on-write protection + "Syncing CHD file…" apply-on-shutdown

Status: **flatten-on-exit (Phases 2–5) implemented 2026-06-18; Phase 1 COW toggle
still pending.** What ships now folds an existing `.diff.chd` back into its base
on a clean app exit, with a "Synchronizing disks…" modal — covering the
compressed-CHD case (which always auto-creates a diff) without the per-disk COW
toggle. See "Implemented (v1)" below.

## Implemented (v1) — flatten-on-exit

- **Core** (`src/chd_disk.rs`): `ChdHd` tracks `base_path` / `diff_path` / `dirty`
  (dirty = wrote this session, or reattached a pre-existing diff). `pending_sync()`
  returns `(base, diff)` when a fold-back is owed. `flatten_diff(base, diff,
  progress, cancel)` rebuilds the base from the merged `reopen_diff` view via
  `create_from_reader` (preserving codecs/geometry/hunk+unit → compressed stays
  compressed) into a `.synctmp.chd`, fsyncs, **atomic-renames over the base**, then
  deletes the diff. Any error/cancel leaves base+diff intact. Unit-tested
  (`flatten_folds_diff_into_compressed_base`, `cancelled_flatten_preserves_base_and_diff`).
- **Plumbing**: `ScsiDevice::{pending_chd_sync, take_pending_chd_sync}` →
  `Wd33c93a::{pending_chd_sync_count, sync_chd_disks}` (rebuild runs outside the
  device lock) → `Machine::{pending_chd_sync_count, sync_chd_disks}`.
- **GUI** (`iris-gui`): `Status.chd_sync_pending` (reported each tick),
  `Cmd::SyncDisks`, `Evt::SyncProgress{disk,total,fraction}` / `Evt::SyncDone`.
  `App::update` intercepts `close_requested`: if a sync is pending it sends
  `SyncDisks`, shows the "Synchronizing disks…" modal (progress bar), and
  `CancelClose`s; the worker stops the machine, flattens with progress, then the
  app closes on `SyncDone`. `sync_then_close` latches so the final close isn't
  re-intercepted.
- **Stop button** also folds: `request_stop()` (the safe-stop choke point both
  Stop buttons use) sends `SyncDisks` instead of `Stop` when safe-to-stop AND a
  sync is pending, with `sync_then_close=false` so the app stays open. (A
  force-stop / unsafe stop keeps the diff — only the safe path folds.)
- **Sandbox flatten — Option A (folder grant), plumbing implemented 2026-06-18.**
  The fold needs to create a temp sibling beside the base and `rename` over it,
  which a per-*file* security-scoped grant can't do. Fix: the user grants the
  *folder* their disks live in (File → "Grant a disk folder…", App Store builds
  only); a *directory* bookmark is recursive, so it covers the base, the
  diff/fold-temp written beside it, AND an NFS shared subfolder under it — one
  grant. Stored as `GuiSettings.disk_folders`, harvested into `bookmarks`
  (directory bookmark) and `restore()`d at launch like file bookmarks. Once the
  folder is granted the existing `flatten_diff` (temp + rename) just works — no
  change to `flatten_diff` or the container diff location needed. **Unverified on a
  signed sandbox build** (the dir-bookmark recursive-write behaviour is
  Apple-documented but untested here).
- **Reveal in file manager** — "📂" button on every path field
  (`config_ui::reveal_in_file_manager`). macOS uses
  `macos_sandbox::reveal_in_finder` → `NSWorkspace selectFile:inFileViewerRootedAtPath:`
  (objc2 raw msg_send, no new dep) so it's **sandbox-safe** (no `open` subprocess);
  Windows/Linux shell out. Works on dev/notarized now; sandbox path believed-good,
  unverified on a signed build.
- **NFS shared folder under the App Store** — both paths: (A) pick any folder in
  the NFS picker (grants it directly), or (B) a "Use <granted>/shared" button per
  granted disk folder that `create_dir_all`s a `shared` subfolder and points the
  share at it (recursive grant flows down). Guidance text shown when no folder is
  granted yet. `disk_folders` is threaded through `show_tab`→`show_network`.
- **Per-disk COW toggle wired to CHDs (Phase 1), 2026-06-18.** The existing
  `overlay` flag is now the universal copy-on-write toggle and is honored for
  CHDs: `ChdHd::open(path, cow)` — COW on → ALWAYS overlay (even an uncompressed
  base gets a diff, so the base is never written in-session) and `pending_sync()`
  returns `None` (never auto-folds); COW off → unchanged (in-place uncompressed /
  auto-folding compressed). `cow commit` and `cow reset` extended to CHDs in
  `ScsiDevice` (commit = `flatten_diff` + reopen; reset = delete the `.diff.chd` +
  reopen a fresh overlay = rollback); the monitor `cow status|commit|reset` is
  backend-agnostic so it now drives CHDs too. Disks-tab checkbox relabelled
  "Copy-on-write". Unit test: `cow_keeps_changes_separate_and_rolls_back`. One
  unified COW concept across raw + CHD — no parallel system.
- **GUI commit / rollback (SCSI menu), 2026-06-18.** When stopped, the SCSI menu
  lists each disk that has an on-disk overlay (`.diff.chd` / `.overlay`) with
  "⬇ Commit changes to disk" and "↩ Discard changes (roll back)". These are
  file-level and machine-independent — `Cmd::CowCommit`/`Cmd::CowReset` run in the
  worker with no machine loaded (so they can't corrupt a running guest; the items
  are hidden/disabled while running). A CHD commit reuses the "Synchronizing
  disks…" progress modal (`flatten_diff` → `SyncProgress`/`SyncDone`,
  `sync_then_close=false` so it doesn't quit); raw commit + all rollbacks are
  instant (`CowDone` → toast). `chd_disk::diff_path_for` is now `pub`. COW default
  remains **off**.
- **Not yet**: a Cancel button on the sync/commit modal; a confirm dialog before
  rollback (it discards the overlay); the in-place incremental flatten for
  uncompressed bases; verifying the folder-grant fold + reveal + shared-folder
  flow on a signed App Store build.

Original plan (still the target for Phase 1) below.

---

Status (original): **plan, not yet implemented** (authored 2026-06-16, revised same
day after design decisions; to implement next session).

## Goal

1. **Copy-on-write (COW) as a per-disk toggle that protects the base image.**
   When on, the emulator never writes directly to the disk during a session —
   changes go to a differencing file — so a crash or force-quit mid-boot can't
   corrupt the base.
2. **On a clean shutdown, apply ("sync") the diff back into the disk**, showing a
   **"Syncing CHD file…"** window (iris-gui) / console line (iris), then exit. So
   the disk "acts like a normal one": edit freely all session; changes fold back
   into the single CHD on clean exit.

Applies to **both** binaries: all disk logic is in the shared `iris` core crate;
only the sync *presentation* differs per frontend.

## Design decisions (locked)

- **D1 — defaults.** Dev/interactive build: **COW ON** by default (devs test and
  force-quit often → protect by default). Release/bundled (App Store) build:
  **COW OFF**, always sync directly to the disk (end users aren't testing; they
  want normal persistent-disk behavior, no overlay to manage). Toggle remains
  user-settable in both. *(Confirm: dev default ON — alternative is keep dev
  default OFF too and make COW purely opt-in.)*
- **D2 — follow MAME, no libchdman-rs extension.** Use MAME's native
  differencing-CHD mechanism for COW (`open_with_diff`/`reopen_diff`), and the
  in-process `chdman copy` equivalent (`hd::create_from_reader` / `copy::copy`)
  to flatten on shutdown. MAME never auto-merges a diff; the *only* thing we add
  is automating that flatten on a clean exit. No new libchdman APIs required.
- **D3 — apply-diff ⇔ COW on.** Applying the diff to the base happens only when
  COW is enabled (the commit-on-shutdown / sync step). COW off = writes go
  straight to the base (in place). Sole MAME-style exception: a *compressed* CHD
  with COW off can't write in place, so writes sit in a diff and stay there
  (flatten manually) — pure MAME, we don't special-case it.

### COW behavior matrix

| Base image | COW ON (protect) | COW OFF (direct) |
|---|---|---|
| Uncompressed CHD | writes → diff; apply diff into base on clean exit | writes in place (MAME) |
| Compressed CHD | writes → diff; recompress base+diff on clean exit | writes → diff, kept/not merged (MAME) |
| Raw image | writes → `.overlay`; apply on clean exit | writes in place |

## Why (recap of findings)

- User's boot disk `irix65.chd` is an **uncompressed** CHD v5 (chdman:
  `Compression: none`), 100.66 GB logical / 5.71 GB physical (sparse, not
  compressed). Uncompressed CHDs are written **in place** today — no diff — so a
  mid-boot kill corrupts the base directly. This feature closes that gap.
- How MAME does writable CHDs (the model we follow): compressed CHD → read-only,
  writes go to an uncompressed differencing CHD (`.dif`) with the original as
  parent; never auto-merged. Uncompressed CHD → in place.
- `libchdman-rs` (pinned `0.287.0-l7`, `prebuilt`) already provides everything
  in-process (no shelling to `chdman`):
  - `HdImage::open(path)` — writable in place; **succeeds only for uncompressed**.
  - `HdImage::open_with_diff(parent, diff)` — open parent read-only, create an
    uncompressed differencing child; writes land in the diff. (`hd.rs:357`)
  - `HdImage::reopen_diff(parent, diff)` — reattach existing diff to parent.
    (`hd.rs:389`) — gives the *merged* view via `read_sector`.
  - `hd::create_from_reader<R: Read>(reader, out, opts, &mut progress, &cancel)`
    (`hd.rs:176`) and `copy::copy(src, dst, opts, progress, cancel)` (`copy.rs:58`)
    — in-process `chdman copy`; `progress`/`cancel` callbacks drive the UI.
  - `HdCreateOptions { logical_size, hunk_size=4096, unit_size=512, codecs:[u32;4],
    geometry, ident }` (`hd.rs:42-63`); codecs `codec.rs`.
- `CowDisk` (`src/cow_disk.rs`) already handles raw-image COW (overlay + dirty
  set + `commit()`/`flush()`/snapshot import/export). **Keep it for raw images.**
  Do *not* generalize it over CHDs — CHDs use MAME's diff instead.

### Assumptions to verify during implementation

- `open_with_diff` accepts an **uncompressed** parent (needed for COW-on over an
  uncompressed CHD, to force a diff where today we'd write in place). MAME diffs
  allow any parent; confirm the libchdman binding does too.
- Flatten path: prefer `reopen_diff(parent, diff)` → read merged sectors →
  `create_from_reader` into a temp CHD with the **original's codecs + geometry +
  GDDD/ident metadata** (read from the source header) → fsync → atomic rename →
  delete diff. (`copy::copy` may need explicit parent resolution for a diff
  source; `create_from_reader` over the merged `reopen_diff` view sidesteps that.)

## Implementation

### Phase 1 — Core: COW toggle wired through MAME's diff

Files: `src/chd_disk.rs`, `src/wd33c93a.rs`, `src/config.rs`.

1. **`ChdHd` open modes** (`chd_disk.rs:36-56`): add an explicit COW-on open that
   **always** uses `open_with_diff`/`reopen_diff` (forces a diff even for an
   uncompressed parent), vs. COW-off which keeps current behavior (`open` in
   place for uncompressed; diff fallback for compressed). Diff path via existing
   `diff_path_for` (honors `IRIS_CHD_DIFF_DIR`).
2. **Dispatch** (`wd33c93a::add_device`, `:327-365`): pass the device's `cow`
   flag into the CHD branch (today it's ignored, `:336-338`). `cow` on → COW-on
   open; off → current behavior. Raw branch keeps `CowDisk` (`:351-357`).
3. **Config** (`config.rs:20-21`): keep field `overlay: bool` (serde back-compat;
   add `cow` alias) as the universal COW toggle, now honored for CHD too. Default
   per D1 (dev on / release off) — gate the default on the build/`appstore`
   feature or a release flag.

### Phase 2 — Core: flatten (apply diff) on demand

New `ChdHd::flatten()` (or a free fn in `chd_disk.rs`):
- Quiesce writes (caller ensures SCSI worker stopped).
- Open merged view `reopen_diff(parent, diff)`; read the original's codecs +
  geometry + ident from the source CHD.
- Stream merged sectors into a temp CHD via `create_from_reader` (same codecs →
  compressed stays compressed, uncompressed stays uncompressed), driving
  `progress(0.0..=1.0)` and honoring `cancel`.
- `fsync` temp → **atomic rename** over the base path → delete the diff.
- On `cancel`/error: delete temp, leave base + diff intact (next launch
  reattaches the diff — nothing lost).
- *Optimization (later, optional):* for an uncompressed base, write only changed
  sectors in place instead of a full rebuild — needs diff-hunk enumeration (a
  small libchdman addition); not required for v1.

### Phase 3 — Shutdown wiring (shared)

- **`Wd33c93a::sync_disks(progress)`** (new): after the SCSI worker thread is
  stopped, walk `devices[]`; for each COW-on CHD device with a non-empty diff,
  call `flatten(progress)`; for raw COW devices call `CowDisk::commit()`.
- **Call site — `Machine::stop()` (`machine.rs:625-630`)**: the single choke
  point both shutdown paths hit — guest soft power-off (`machine.rs:608`, just
  before `process::exit(0)` at `:614`) and host window-close (`ui.rs:642` →
  `main.rs:112`). No-op when nothing needs flattening.

### Phase 4 — iris core presentation

- `eprintln!("iris: Syncing CHD file… ({} of {})", done, total)` around the
  flatten in `Machine::stop()`, matching the `println!("Machine: soft power-off")`
  style (`machine.rs:607`) and `cow_disk.rs:239`. Console only.
- On-screen overlay is out of scope for the core binary (Rex3 refresh thread is
  torn down by `rex3.stop()` inside `stop()`; power-off exit runs off the UI
  thread). A graphical message would need a `StatusBar` message field
  (`disp.rs:480`) + a forced final `present()` before `machine.stop()`.

### Phase 5 — iris-gui presentation ("Syncing CHD file…" window)

`iris-gui/src/main.rs` + `iris-gui/src/handle.rs`:
1. **Worker command** (`handle.rs`): add `Cmd::SyncDisks` (`:11-22`), handled in
   `worker_loop` (`:172-322`) modeled on `Cmd::SaveState`'s background disk work
   (`:264-282`). It calls `machine.sync_disks(|p| Evt::SyncProgress(p))` and ends
   with `Evt::SyncDone`. Add `Evt::SyncProgress(f32)`/`Evt::SyncDone`, merged in
   `drain_events` (`:106-126`).
2. **Close interception** (`App::update` top, `:1318-1320`): detect
   `ctx.input(|i| i.viewport().close_requested())`; if any COW device has a
   pending diff and sync not started → send `Cmd::SyncDisks`, set
   `self.syncing = Some(..)`, `ctx.send_viewport_cmd(ViewportCommand::CancelClose)`.
   Route File→Quit (`:580-582`) through the same path.
3. **Modal** (inline `Option` style like `stop_modal` `:1451-1471`):
   `egui::Window::new("Syncing CHD file…").collapsible(false).resizable(false)`
   `.anchor(CENTER_CENTER,..)` with a label + `egui::ProgressBar::new(job.progress)`
   (+ optional Cancel → worker cancel flag, keeps the diff). `ctx.request_repaint()`.
4. On `Evt::SyncDone`: `self.syncing=None;` then `ViewportCommand::Close` → falls
   through to `on_exit` (`:1590-1602`) → `emu.shutdown()` (`:1598`).
5. **`safe_stop.rs`** (`:41-61`): with COW, force-stop never touches the base and
   the diff persists + reattaches next launch, so reword "Confirm stop" copy:
   not "may corrupt base" but "changes aren't yet synced into the disk; they're
   kept in the overlay and applied on the next clean exit."
6. **COW toggle UI** (Disks tab): a per-device checkbox **"Protect base image
   (copy-on-write)"** with the help/protection blurb below. Default per D1.

### COW explanatory text (UI + docs)

> **Protect base image (copy-on-write)**
> When on, the emulator never writes directly to this disk during a session — all
> changes go to a temporary overlay, so the original image stays intact even if
> the emulator crashes or is force-quit mid-boot. On a **clean shutdown** your
> changes are merged ("synced") back into the disk. When off, changes are written
> straight to the disk as you go, like a normal hard drive.
> *Recommended on while testing or experimenting; off for everyday use.*

## Edge cases & safety

- **Crash / kill mid-session:** base untouched; diff persists and reattaches next
  launch (`reopen_diff`). Nothing committed → nothing corrupted.
- **Crash mid-flatten:** temp-file + atomic rename → original intact on
  interruption; diff only deleted after a successful rename.
- **Disk space:** flatten needs room for a second copy of the CHD; check free
  space first and surface a clear error in the modal.
- **Large logical disks:** `irix65.chd` is 100 GB logical. A full rebuild streams
  the whole logical size (sparse-zero-dominated, fast-ish); progress bar covers
  it, Cancel aborts safely. (The later in-place optimization avoids the rebuild
  for uncompressed bases.)
- **Snapshots / CI:** raw-image overlay format unchanged → `export/import_overlays`
  (`machine.rs:880,1008,1245`) keep working. CHD diffs are not part of snapshot
  capture today; confirm snapshot save/load still behaves with a COW-on CHD
  (likely: snapshot the diff path or require flatten before snapshot).

## Test plan

- Unit (`cargo test --lib --features chd`):
  - COW-on over uncompressed CHD: writes go to diff, base bytes unchanged; after
    flatten, base reflects writes and is still a valid uncompressed CHD; diff gone.
  - COW-on over compressed CHD: same, output still a valid **compressed** CHD
    (`Chd::verify`/reopen), same logical size + preserved geometry/metadata.
  - Cancelled flatten: base + diff intact, reattach works.
- Manual (bounded runs, clean `halt`, per `dont-run-iris-long`):
  - iris-gui: boot `irix65.chd` COW-on, edit, clean shutdown → modal + progress →
    changes present next boot. Force-stop mid-session → base unchanged, diff
    reattaches.
  - iris core: same via console `Syncing CHD file…` line.

## Out of scope / future

- In-place incremental flatten for uncompressed bases (needs diff-hunk
  enumeration in libchdman).
- Graphical sync overlay in the core `iris` binary.
- Periodic/background flatten during a session (today: clean exit or explicit
  monitor `cow commit`, which already exists at `wd33c93a.rs:876-926`).
```
