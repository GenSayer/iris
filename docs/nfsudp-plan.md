# Plan: in-core NFSv3-over-UDP server (`src/nfsudp.rs`)

Status: **planning** — no code yet. Replaces the external `unfsd` with a
synchronous, pure-Rust NFS/UDP server that lives inside the NAT.

## Decisions (locked 2026-06-18)

- **Support both NFSv2 (IRIX 5.3) and NFSv3 (IRIX 6.x).** Best done by handling
  *both at once*: the server dispatches on the RPC `vers` field, so the guest's
  own `mount` picks the version (5.3 → v2, 6.x → v3) and we reply in kind — no
  dropdown needed for it to work. We'll still expose an optional **NFS version:
  Auto / v2 / v3** config (default Auto) to force/limit it for testing.
  Implementation: MOUNT v1 (for NFSv2) + MOUNT v3, and NFS v2 + v3 procedure
  sets, all over a **shared backend** (path↔id map, file I/O, faked attrs); only
  the XDR/attr encoding differs per version.
- **Full read-write in one pass** (not a read-only spike). Implies the
  **duplicate-request cache** (Q14) is in scope from the start.
- **Inbound IP reassembly now** (Q2): add a fragment-reassembly buffer to
  `handle_ip` so large `wsize` works — fast writes, symmetric with reads.
- **Simplest synthetic permissions** (Q5): fixed uid/gid 0, mode by heuristic
  (dir 0755 / file 0644 / +x if detectable), accept-and-ignore the guest's
  SETATTR chmod/chown.
- **Guest side / `.rhosts` note:** NFS mounting needs **no `.rhosts`** (that file
  is rsh/rcp trust, not NFS). The only guest-side step is the `mount` command,
  which the GUI already emits live. The "add host to `.rhosts`" instruction
  belongs to the *rcp/rsh* file path, which is separate — see clarification
  request in the chat.

## Goal & hard constraints

- A minimal **NFSv3 server, UDP only**, in `src/nfsudp.rs` (the `iris` core
  crate). No TLS, no NLM/locking, no real auth (allow every host, ignore RPC
  credentials), **faked/synthesized unix permissions** for cross-platform
  parity (esp. Windows, which has no unix uid/gid/mode).
- **The whole protocol stays inside the NAT.** The NAT engine dispatches the
  guest's NFS/MOUNT/portmap RPC straight to the in-core server and injects the
  replies as virtual-network frames. **Zero host network sockets** for NFS. The
  *only* thing that touches the host is **file I/O to the user-chosen backing
  folder**.
- Result: kills the `unfsd` problems (no native Windows build, no Homebrew, no
  spawning an external binary in the macOS App Store sandbox) and lets us
  **un-gate NFS on every platform**.

## Why it fits the existing NAT cleanly

`src/net.rs` already does most of the wiring:
- **Portmap** is intercepted in-NAT (`handle_portmap_udp`, `portmap_lookup`/
  `portmap_reply`) — it answers GETPORT with `NFS_VM_PORT=2049` / `MOUNTD_VM_PORT
  =1234`. That's our "minimal service discovery"; reuse as-is.
- **NFS/mountd** UDP currently falls through `handle_udp`'s default arm to
  `nfs_remap_dst`/`nat_udp` (forward to `unfsd` on host loopback). We **replace
  those two cases** with: hand the RPC payload to `Nfsv3Server`, inject the reply.
- **Outbound fragmentation is already solved** — `ip_frames_udp` /
  `ip_fragment_frame` fragment a large UDP reply across Ethernet frames; the
  guest reassembles. So large READ replies work out of the box.
- **Inbound fragmentation is NOT handled** — `handle_ip` treats each frame as a
  whole datagram (no MF/offset reassembly). So large guest WRITEs would break.
  v1 mitigates by advertising a small `wtmax` (see below).

## Architecture

```
guest ──UDP RPC──▶ NAT (net.rs handle_udp)
                     ├─ port 111  → handle_portmap_udp  (already)
                     ├─ port 1234 → Nfsv3Server::mount_call(payload)  ◀ new
                     └─ port 2049 → Nfsv3Server::nfs_call(payload)    ◀ new
                            │ reply bytes
                            ▼
                     ip_frames_udp(...) → enqueue_rx (inject, auto-fragmented)
Nfsv3Server ──std::fs──▶ <backing folder on host disk>   (the ONLY host I/O)
```

- `Nfsv3Server` is **synchronous** (no tokio): `fn nfs_call(&mut self, call:
  &[u8]) -> Vec<u8>` and `fn mount_call(...)`. Given one RPC call datagram,
  produce one reply datagram. The NAT owns it (`Option<Nfsv3Server>` in
  `NatEngine`/`GatewayConfig`), created at machine start from the NFS config.
- No `start_unfsd`, no loopback ports, no `unfsd` binary.

## What to reuse from `../nfsserve` (BSD-3-Clause)

Vendor (copy, with attribution) the **transport-agnostic** pieces; write our own
sync dispatch + backend. Do **not** depend on the crate (it pulls tokio +
async-trait, which we don't want in-core).
- `xdr.rs` — XDR encode/decode primitives (RFC 1014).
- `nfs.rs` / `mount.rs` — the wire structs (`fattr3`, `sattr3`, `fhandle3`,
  `diropargs3`, etc.) and constants.
- `nfs_handlers.rs` / `mount_handlers.rs` — reference for each procedure's
  semantics (re-implemented synchronously).
- `examples/mirrorfs.rs` — model for the local-dir backend + the path↔id map.

## RPC + procedures

- **RPC layer** (RFC 1057): parse call (xid, rpcvers=2, prog, vers, proc;
  ignore cred/verf), build accepted reply (success / nfs error). Programs:
  MOUNT `100005 v3`, NFS `100003 v3`.
- **MOUNT v3**: NULL, MNT (any path → the single export root fh), UMNT (no-op),
  EXPORT/DUMP (minimal/optional).
- **NFS v3**: NULL, GETATTR, SETATTR, LOOKUP, ACCESS (grant all), READLINK,
  READ, WRITE, CREATE, MKDIR, REMOVE, RMDIR, RENAME, READDIR, READDIRPLUS,
  FSSTAT, FSINFO, PATHCONF, COMMIT (no-op — we write through). Defer: MKNOD,
  LINK, SYMLINK.

## Backing store (the only host interaction)

- One export = the user's chosen folder. Root fileid = 1.
- **path↔fileid map** in memory (like mirrorfs's `FSMap`): assign sequential
  64-bit ids; map id↔relative path. **Don't** use the host inode (absent/unstable
  on Windows).
- **File handles**: opaque `fhandle3` encodes the 8-byte fileid.
- **Faked `fattr3`**: synthesize `mode` (dir 0755 / file 0644, +x by heuristic),
  `uid`/`gid` (fixed, e.g. 0 — configurable), `nlink`, `size`/times from host
  metadata when present else `now`, `fileid`, fixed `fsid`, `rdev`=0. On Windows,
  fully synthetic.
- **Path containment**: every op resolves within the export root; reject `..`
  escapes and symlinks that leave the root. (No NFS auth, but containment is
  mandatory.)

## Fragmentation strategy

- **READ (outbound): large is fine** — advertise `rtmax`/`rtpref` ~8 KB (or
  more); `ip_frames_udp` fragments, guest reassembles.
- **WRITE (inbound): avoid fragmentation in v1** — advertise small `wtmax`/
  `wtpref` (~1 KB, fits one frame). Correct but slow writes; no NAT reassembly
  needed.
- **v2 perf**: add inbound IP reassembly to `handle_ip` (reassembly buffer keyed
  by src/id/proto) → allow large `wsize`.

## Config / GUI changes

- `NfsConfig`: keep `shared_dir`; **drop `unfsd`, `nfs_host_port`,
  `mountd_host_port`** (no binary, no loopback). Optional: faked `uid`/`gid`.
  Migrate old configs via serde defaults / ignore-unknown.
- **Un-gate NFS** in `config_ui.rs` (remove the Windows / macOS-bundled gating —
  it now works in-process everywhere, including the sandbox). Drop the "unfsd
  binary" field + macOS install hint.
- Mount hint stays **UDP** (IRIX default): `mount <gateway>:/ /shared` (force
  `vers=3`). Replace `start_unfsd` in `main.rs` with constructing `Nfsv3Server`
  and handing it to the NAT.

## Phasing

- **A — read-only**: MOUNT MNT + NULL/GETATTR/LOOKUP/ACCESS/READ/READDIR(PLUS)/
  FSINFO/FSSTAT/PATHCONF. Goal: IRIX mounts and reads files.
- **B — read-write**: SETATTR/WRITE/CREATE/MKDIR/REMOVE/RMDIR/RENAME/COMMIT (+ a
  duplicate-request cache, see Q14).
- **C — perf/extras**: inbound reassembly → large `wsize`; symlinks; tidy.

---

## Open questions (flagged together)

1. **NFSv2 vs v3 — biggest unknown.** Will IRIX 6.5 mount **v3** cleanly when we
   force `vers=3`, or does it default to / fall back to **v2**? If it insists on
   v2 we'd need v2 procedures too (different XDR + attrs) — a real scope bump.
   *Needs a real-boot test.*
2. **Inbound WRITE fragmentation.** Ship v1 with small `wtmax` (~1 KB, no
   reassembly) and accept slow writes, or add inbound IP reassembly up front?
3. **rsize/wsize floors.** Does IRIX 6.5 actually honor a small advertised
   `wtmax`/`rtmax`, or does it have a minimum it uses regardless? *Real-boot.*
4. **Blocking I/O on the NAT thread.** The server does synchronous `std::fs` on
   the NAT thread (bounded per RPC). Acceptable, or offload to a worker
   thread/queue so a slow disk can't stall other NAT traffic?
5. **Faked-perms policy.** Fixed `uid`/`gid` (0? configurable?), `mode` heuristic
   (dir 0755 / file 0644 / +x how?), and what to do with the guest's SETATTR
   chmod/chown — keep in the in-memory map, persist to a sidecar, or accept-and-
   ignore (esp. Windows)?
6. **File-handle stability.** In-memory path↔id map → handles change across IRIS
   restarts. The guest remounts on *its* reboot so it's probably fine — but do we
   ever need handles stable across an IRIS restart (persist the map)?
7. **Filename encoding.** NFS filenames are opaque bytes; host paths are UTF-8
   (mac/Linux) / UTF-16 (Windows). What's IRIX's filename charset, and how do we
   map non-UTF-8 names cross-platform?
8. **Symlinks.** Support READLINK/SYMLINK? Windows symlink creation is
   limited/privileged — fake, skip (NOTSUPP), or best-effort?
9. **Special files (MKNOD).** IRIX may try to create device nodes/FIFOs (e.g.
   extracting an archive). Return NOTSUPP, or handle?
10. **Read-only v1?** Ship read-only first (safe, smaller), or go straight to
    read-write?
11. **Symlink escape policy.** A symlink in the export pointing outside it —
    follow (leak) or refuse? (Containment.)
12. **MOUNT export path.** Accept any path in MNT → the single export root, or
    honor sub-path mounts?
13. **Vendor vs depend.** Confirm: vendor nfsserve's XDR/NFS/MOUNT structs into
    `nfsudp.rs` (sync, no tokio), with BSD-3 attribution — rather than depend
    on the crate.
14. **Duplicate-request cache (DRC).** NFS-over-UDP retransmits on timeout; non-
    idempotent ops (WRITE/CREATE/REMOVE/RENAME) need a small DRC keyed by
    `(xid, src)` to avoid double-apply. In scope for v1 write support?
15. **fsid.** What `fsid` to present (fixed value)? Does IRIX care?
16. **IRIX quirks.** Any IRIX-6.5-specific attribute/poll quirks to expect
    (nfsserve's README notes some clients poll oddly with old protocols)?
17. **Config migration.** Old saved configs carry `unfsd`/`nfs_host_port`/
    `mountd_host_port`; drop them without breaking deserialization.
18. **NAT thread ownership.** `Nfsv3Server` lives in/with `NatEngine` (NAT
    thread). The GUI sets the backing dir at machine start (config); do we ever
    need to change the dir live (like the subnet/forwards), or is start-time
    enough?
