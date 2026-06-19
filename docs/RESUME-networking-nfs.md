# Resume / troubleshooting handoff — networking + in-core NFS

> Read this first to continue work on the networking stack and the in-core NFS
> server. Everything below is on branch **`iris-gui-net-indicator`** and is
> **committed but not pushed**. All of it **builds clean and unit-tests pass**,
> but the runtime behavior is **UNVERIFIED against a real IRIX boot** — that's
> the main remaining task.

## Quick state

```
cargo build                          # whole workspace — clean (only pre-existing coffdump warnings)
cargo test -p iris nfsudp            # 19 NFS unit tests
cargo test -p iris net::             # NAT unit tests (incl. ftp_alg_tests)
cargo test -p iris-gui               # 28 GUI tests (netplan etc.)
cargo build -p iris-gui --features appstore   # sandbox build also clean
```

Untracked, intentionally NOT committed: `PR_DESCRIPTION.txt`, `docs/cow-chd-sync-plan.md`.

Two `../` siblings referenced: `../nfsserve` (BSD-3 reference impl we modeled the
NFS structs on; we did NOT depend on it — re-implemented synchronously).

## What was built (two streams, see `git log --oneline`)

**A. Networking tab + NAT diagnostics + live reconfigure** (commits `a2b1c51` →
`7ca5811`):
- `iris-gui/src/netplan.rs` — pure subnet logic (CIDR, mask presets, first-free
  per RFC1918 block via `if-addrs`, conflict detection, snap-to-network, sanity
  tiers). Unit-tested.
- Networking config tab rewrite (`config_ui.rs`): network preset + mask dropdowns,
  derived line, override modal, Add-forward menu (Telnet/FTP/Custom), NFS section.
- Check-networking window (`main.rs network_check_window`): explains the
  plug-and-play adoption model, offers "Set IRIS's NAT subnet to <guest>/24"
  (applies live), errors if the guest subnet overlaps a host network, and hints
  `chkconfig network on`.
- `src/net.rs`: **live NAT subnet apply** (`NatControl::request_subnet` +
  `apply_subnet` flag → NAT thread swaps config + flushes), **host-conflict
  guard** on plug-and-play adoption (`host_conflict` / `set_host_nets`), **FTP
  passive-mode ALG** (`ftp_pasv_rewrite` + transient data forwards), **live
  port-forward rebind** (`request_subnet`-style `apply_forwards`), and an
  **adoption robustness fix** (gate on `routed` = a frame to the gateway MAC, not
  on any IP frame — so a broadcast ping no longer disarms adoption).

**B. In-core NFS/UDP server** (commits `4108070` → `43c1136`) — the main recent
work. Replaces external `unfsd`. See next section.

## NFS server architecture (the thing to troubleshoot)

**Goal:** pure-Rust NFSv2+v3 over UDP, **entirely inside the NAT, zero host
sockets**. Only host I/O is the backing folder. Plan + 18 open questions:
`docs/nfsudp-plan.md`.

**`src/nfsudp.rs`** (all unit-tested, no tokio):
- `NfsBacking` — one export folder + a `fileid`↔relative-path map; synthetic
  ("faked") attrs (uid/gid 0, mode heuristic dir 0755 / file 0644 +x on unix);
  path-contained file ops (lookup/attr/readdir/read/write/create/mkdir/remove/
  rmdir/rename/truncate).
- XDR (`Xdr`/`Cur`) + RPC (`parse_call`/`reply`) — big-endian, one datagram = one
  RPC message.
- NFSv3 (`nfs3_call`): NULL/GETATTR/SETATTR/LOOKUP/ACCESS/READ/WRITE/CREATE/MKDIR/
  REMOVE/RMDIR/RENAME/READDIR/READDIRPLUS/FSSTAT/FSINFO/PATHCONF/COMMIT. `fattr3`
  uses **full st_mode** (type+perm, like knfsd).
- NFSv2 (`nfs2_call`): 32-bit `fattr`, fixed 32-byte handles, microsecond
  timevals, v2 proc numbers (these DIFFER from v3 — see `PROC2_*` vs `PROC3_*`).
- MOUNT (`mount_call`): v1 + v3; MNT returns the root handle; export path ignored
  (single export "/"); EXPORT advertises "/".
- `NfsServer { backing, drc, version }` — `handle(datagram) -> Option<reply>`:
  parse, DRC check (non-idempotent ops only, by xid, 256-entry FIFO), dispatch by
  `prog` (MOUNT 100005 / NFS 100003) and `vers` (honoring `NfsVersion::Auto/V2/V3`).

**`src/net.rs`** wiring:
- `NatEngine` owns `nfs: Option<NfsServer>`, built in `new()` from `config.nfs`
  (`shared_dir` + `version`).
- `handle_udp`: `NFS_VM_PORT (2049) | MOUNTD_VM_PORT (1234)` to the gateway →
  `handle_nfs_udp` → `NfsServer::handle` → reply injected via `udp_packet` +
  `ip_frames_udp` (auto-fragments large reads) into `deferred_rx`.
- **portmap** (UDP 111) already answered in-NAT by `handle_portmap_udp` /
  `portmap_lookup` (returns 2049 for NFS, 1234 for mountd, 111 for portmap).
- **Inbound IP reassembly** in `handle_ip` (`FragReasm`, keyed by
  (src_ip, ip_id, proto), 5s eviction) — needed for large NFS WRITEs.
- External `unfsd` fully removed (CLI `start_unfsd`, `NfsConfig` unfsd/host-port
  fields, the dead branches in `nfs_remap_dst`/`nfs_unmap_src`).

**Config/GUI:** `NfsConfig { shared_dir, version }` (`src/config.rs`); GUI NFS
section (`config_ui.rs`) is un-gated on all platforms, has a shared-dir picker +
an Auto/v2/v3 dropdown + a live mount hint `mount <gateway>:/ /shared`.

## End-to-end flow (what should happen on the wire)

1. Guest sends portmap GETPORT (UDP→gateway:111) for MOUNT/NFS → NAT replies with
   1234 / 2049.
2. Guest sends MOUNT MNT (UDP→gateway:1234) → `mount_call` returns the root fh.
3. Guest sends NFS ops (UDP→gateway:2049) → `nfs3_call`/`nfs2_call`.
4. Replies come from gateway:1234 / gateway:2049 back to the guest's source port.

## How to validate + troubleshoot on a real boot

1. **Enable NFS**: GUI → Configuration → Networking → NFS share → pick a folder.
   Make sure networking is up in the guest (`chkconfig network on`, default route /
   plug-and-play adoption).
2. **Turn on NAT logging** to see the dispatch: the code uses
   `dlog_dev!(LogModule::Net, ...)`. Enable Net logging (GUI Debug/JIT tab debug-log
   field, or `IRIS_DEBUG_LOG`) — look for `NAT portmap`, `NAT UDP ... → ...:2049`,
   and add temporary `eprintln!` in `handle_nfs_udp` / `NfsServer::handle` if needed.
3. **Mount from IRIX**: `mkdir /shared; mount <gateway>:/ /shared; ls /shared`.
   (`<gateway>` is shown in the Check-networking window; default 192.168.0.1.)

### Likely failure points (ranked) and where to look

- **portmap mismatch** — the guest may query MOUNT **v1** (for v2) vs **v3**;
  `portmap_lookup` (`src/net.rs`) should return the port regardless of version.
  Confirm it returns 1234 for prog 100005 and 2049 for 100003. If the guest can't
  find mountd, the mount never starts.
- **MOUNT MNT format** — v1 result is `fhstatus`(u32 status)+32-byte fhandle; v3 is
  `mountstat3`+`fhandle3`(opaque)+auth flavors. If IRIX rejects the mount, re-check
  `mount_call`.
- **fattr quirks** — `fattr3.mode` sends full st_mode; if IRIX shows wrong file
  types, try perm-bits-only. v2 timevals are microseconds (`put_time2` divides
  nsec/1000). v2 sizes are 32-bit.
- **rsize/wsize** — `nfs3_fsinfo` advertises RTMAX/WTMAX = 32768. If IRIX ignores
  this and writes huge datagrams, the **inbound reassembly** (`handle_ip`
  `FragReasm`) must stitch them — verify a large file write. If reads are corrupt,
  check outbound fragmentation (`ip_frames_udp`).
- **reply source port / checksum** — `handle_nfs_udp` sets the reply source port to
  the request's dest port (2049/1234) and `udp_packet` computes the UDP checksum;
  an RPC client may drop a reply from the wrong port or a bad checksum.
- **READDIR cookies** — `nfs3_readdir`/`nfs2_readdir` page by a name-sorted index;
  if `ls` loops or truncates, inspect the cookie/eof logic.
- **DRC over-dedup** — keyed by xid only (256 FIFO). If a legit non-idempotent op is
  wrongly skipped, widen the key (add src) — see `is_idempotent` / `Drc`.

### Tactics
- The protocol layer is fully isolated and testable: add a failing case to
  `mod tests` in `src/nfsudp.rs` that builds the exact bytes IRIX sends (capture
  via a temporary `eprintln!("NFS call {:02x?}", payload)` in `handle_nfs_udp`).
- `../nfsserve` is the reference for any encoding you're unsure about (BSD-3).
- The whole NFS path is synchronous on the NAT thread — a panic there would be
  caught? No: it would kill the NAT thread. Keep handlers panic-free (they use
  `Option`/`else return garbage`).

## Known limitations / decisions (don't re-derive)

No NLM/locking; no auth (every host allowed, creds ignored); faked unix perms
(SETATTR chmod/chown accepted-and-ignored, size honored); single export, MNT path
ignored; READDIR paginated by sorted index; no symlinks/MKNOD/LINK; COMMIT is a
no-op (writes are synchronous); DRC by xid only. NFSv2 (IRIX 5.3) + NFSv3 (IRIX
6.x) dispatched by RPC version (Auto by default).

## Also unverified from stream A (may need the same real-boot debugging)

The FTP passive-mode ALG, live NAT subnet apply, live port-forward rebind, and the
adoption-robustness fix are all RFC/architecture-faithful but **not confirmed on a
real boot**. The FTP ALG only matters for an external FTP client through the
port-forward (the in-app file bridge — Phase 3 of `docs/networking-tab-redesign.md`
— was deferred in favor of NFS; the `docs/suppaftp-emu-fork-prompt.md` spec is on
ice).

## Reference files
- `docs/nfsudp-plan.md` — full NFS plan + 18 open questions.
- `docs/networking-tab-redesign.md` — networking phases + decisions.
- `docs/suppaftp-emu-fork-prompt.md` — deferred FTP-bridge fork spec.
- Memories: `nfsudp-server.md`, `networking-tab-redesign.md`.
