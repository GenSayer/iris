# VINO capture on IRIX 6.5.22 — campaign progress

Companion to [indycam-end-to-end-capture.md](indycam-end-to-end-capture.md)
(which got capture working on **IRIX 5.3**). This note covers making it work
on **6.5.22**, where it currently does NOT fully work yet.

## 5.3-vs-6.5 differential (the key diagnostic) + geometry-fix attempt

Ran the STOCK `/usr/sbin/vidtomem` on BOTH: it **succeeds on 5.3** ("saved image
to file") and **hangs on 6.5** — so the custom client was never the issue.

Diffing the VINO register traces of the two:
- **5.3 (works):** `INTR_STATUS -> 0x01` (EOF only). videod re-programs
  `NEXT_4_DESC` per field (page-stepped blocks, e.g. 0x0852c000 → 0x0852b000),
  EOF-driven, iris never reaches a STOP descriptor.
- **6.5 (hangs):** `INTR_STATUS -> 0x05` (DESC|EOF). videod sets up a dense
  JUMP-chained descriptor table; iris follows it and hits a STOP descriptor,
  raising DESC, which 6.5's videod treats as a (half-captured) done transfer.

Attempted fix (matches MAME's contiguous model): removed the per-row interleave
skip + stride pad in `dma_emit_dword`/`render_and_pump`, and rewound the
descriptor cursor only on the even field so it flows across both fields to the
frame's STOP. Result: even fields now raise EOF (0x01) like 5.3, odd fields hit
STOP (0x05) after the full frame — but **videod still did not deliver a frame to
the client.** So the STOP/EOF timing is necessary-but-not-sufficient; the
blocker is in how 6.5's videod drives/polls the capture (it keeps re-arming per
field and never completes a client transfer). REVERTED (didn't fix 6.5, and it
touches the working 5.3 geometry).

## ROOT CAUSE (via disassembly of the 6.5 kernel vino driver)

Pulled `/var/sysgen/boot/vino.o` (ELF MIPS-III, **not stripped**) off the guest
and disassembled it with capstone (`/tmp/dv.py`). The driver has TWO
descriptor-chain builders, selected by `vinoBuildDAPS`:
`vinoBuildNormalDAPS` vs **`vinoBuildJumpBugDAPS`** — a workaround for an
early-VINO hardware bug in the **4-at-a-time DMA descriptor-cache fetch**.

`vinoBuildJumpBugDAPS` lays out descriptors so that **every 4th slot is a JUMP**
(`0x40000000 | kvtophys(next)`), and the jump target is deliberately offset by
**+4 or +8** (the +8 case skips a descriptor slot) to dodge the buggy fetch.
This is exactly the `0x4861e014`/`0x4861e024`… (`+0x14`, i.e. group+4) layout
seen in the descriptor table at `0x0861e000`.

iris's descriptor engine (`shift_descriptors`/`dma_emit_dword` in `vino.rs`)
does not replicate this jump-bug fetch/skip behavior, so it traverses the chain
incorrectly — wrong pages and/or wrong STOP timing — and the capture never
completes cleanly: iris raises DESC (`INTR_STATUS=0x05`) where 5.3's
normal-DAPS path raises EOF (`0x01`), and the `vinoWakeupTimeout` watchdog
restarts forever. 5.3's older driver always uses the normal layout, which iris
handles — hence 5.3 works.

DISPROVEN: the jump-bug path is **not** gated on board revision. Forcing MC
SYSID rev 0→3 (board_rev 3) did NOT switch the driver to NormalDAPS — the
descriptor table was still the jump-bug `+0x14` layout. The selector
(`vinoBuildDAPS` arg0 = a DMA-geometry value, not board_rev) chooses jump-bug
based on the buffer layout, so it's effectively always on for this capture.

## Next step to actually fix it

Implement correct VINO jump-bug descriptor-cache traversal in `vino.rs`:
fetch descriptors 4-at-a-time, honor JUMP (bit 30) targets as PHYSICAL
addresses with the +4/+8 group offset, and skip the dummy slot exactly as
`vinoBuildJumpBugDAPS` intends, so the chain reaches its terminating STOP only
after a full frame and the captured pages are correct. The authoritative spec
is `vinoBuildJumpBugDAPS` (0x4af4 in vino.o) — disassemble it (`python3
/tmp/dv.py vinoBuildJumpBugDAPS`) to derive the exact +4-vs-+8 condition. This
is a bounded but non-trivial emulator feature; not yet implemented.

## FIXES IMPLEMENTED (verified) + remaining videod blocker

Disassembling vino.o pinned the descriptor differential, and chainwalk on the
guest verified it numerically:
- iris/MAME follow a JUMP target unaligned (`& 0x3fffffff`). VINO actually
  fetches descriptors in **16-byte-aligned groups of four**, so the jump-bug
  workaround's +4/+8 low-bit offsets must be masked. Walking the 6.5 chain
  unaligned yields **181** data pages → premature STOP; **16-byte-aligned**
  yields exactly **300** pages = a full 640×480×4 frame.

Two fixes in `vino.rs` (both verified, NO 5.3 regression — stock `vidtomem`
still saves a frame on 5.3 with them):
1. `shift_descriptors`: mask JUMP target to `& 0x3FFF_FFF0` (16-byte align).
2. interlace geometry: drop the per-row stride skip/pad in
   `dma_emit_dword`/`render_and_pump`, and rewind the descriptor cursor only on
   the even field (`pump_field`), so the cursor flows across both fields and
   reaches the frame's STOP once per frame.

Result on 6.5: the kernel driver now gets **two EOF interrupts per frame**
(`INTR_STATUS=0x01`), exactly like 5.3 — no more premature DESC — and VINO DMA
writes a full frame to RAM (verified by reconstructing 300 pages; content
matches the live camera, including black when the camera is dark).

**BUT `vidtomem` still hangs on 6.5** — with the camera AND with test_pattern.
Since 6.5's VINO interrupt behavior now matches 5.3's (which delivers), the
remaining blocker is **in 6.5's videod daemon**, not the VINO descriptor/
interrupt layer. Leading suspect: 6.5's videod requires valid **UST/MSC frame
timestamps** (`vinoGetUSTMSCPair`/`vinoCorrectUST`/`vinoGetFrontierMSC` in
vino.o) that iris doesn't provide, where 5.3's simpler path didn't. That's the
next thing to emulate. The two vino.rs fixes are correct and worth keeping
regardless.

## DELIVERY MECHANISM (from the ISR disassembly) — the remaining piece

Disassembled `vinoInterrupt` (the ISR). On each interrupt it calls
`update_ust`/`get_ust_nano` (UST timestamp) then dispatches:
```
andi $v1, $s3, 4     ; bit 2 = CHA_DESC
beqz $v1, ...        ; if NOT desc, skip
... -> vinoEOD        ; frame delivery (vinoGetNextBuffer + fill dmrb ring)
```
So **frame delivery (`vinoEOD`) is triggered by the DESC (end-of-descriptor)
interrupt — bit 2 — NOT by EOF.** `vinoEOD` is what hands the completed buffer
to videod's dmedia ring (and `vinoFillInfo`→`dmrb_timestamp` stamps it). EOF
(bit 0) only updates the field/UST state.

This means: for a VL client to get a frame, iris must raise **exactly one DESC
per complete interleaved frame** — after BOTH fields are captured into the
300-page buffer and the cursor reaches the chain's terminating STOP descriptor.

- The **jump-align fix is necessary** (without it the chain hits STOP at 181
  pages, a malformed half frame). KEPT.
- The earlier "cursor-flow / no-skip" change was the WRONG direction: it made
  capture EOF-only (no DESC), so `vinoEOD` never fired. REVERTED.
- The right fix is an interlace-frame restructure: one DMA pass writes even
  rows (at even offsets) and odd rows (at odd offsets) across the 300-page
  buffer, raising EOF at each field boundary and **one DESC at the frame's end**
  (the STOP descriptor). iris currently pumps fields sequentially and the
  per-row interleave skip makes the EVEN field's cursor reach STOP alone
  (premature half-frame DESC). Getting exactly-one-DESC-per-frame from iris's
  field-at-a-time VideoSource is the remaining (intricate) work — NOT done.

UST is likely fine (the ISR's `get_ust_nano` runs as real kernel code over
iris's advancing timers); the blocker is the DESC/interlace-frame timing above,
not the timestamp.

## Bottom line (honest)

The `0x40000000` alias fix made VINO capture **engage** on 6.5 (DMA runs,
DESC/EOF interrupts fire, the driver reads them, camera data reaches RAM). But
**no recognizable frame has been produced yet**: (1) the VL client
(`vinograb`) never gets a frame from videod — `vlGetNextValid`/`vlGetLatestValid`
return NULL (videod isn't filling the client buffer); (2) a `/dev/mem`
reconstruction hack (`vinodump.c`) pulls the bytes out of the DMA pages but does
not correctly model the interleave + row-stride + descriptor-page geometry, so
the reassembled image is scrambled, and the colour is off (a YUV→RGB / U-V-swap
cast turns a cream wall into uniform blue-gray). A real macOS camera grab
(`/tmp/camgrab.swift`) shows the true scene for comparison; the iris output does
not match it. Do not present the current reconstruction as a faithful grab.

## Status

- Enumeration works: `vlinfo` shows `vino 0`, `extensions = EXT_camera`, the
  digital (IndyCam) + analog sources, and Memory Drain nodes. The I2C/CDMC
  camera-attach probe succeeds on 6.5 with the existing emulation.
- A VL capture program (`vinograb.c`, repo root) compiles with MIPSpro `cc`
  (`cc -o vinograb vinograb.c -lvl`), opens the path, negotiates 640×480, and
  the driver **does** program descriptors and enable DMA
  (`CONTROL <- 0xf8e`: DMA+interleave+sync+D1/camera+RGB).
- **VINO DMA actually writes captured pixels to RAM** — dumped a descriptor
  page (`0x0a658000`) mid-capture and found real ARGB data (`ffc9d6db…`).
- **But `vlGetNextValid` times out**: the driver enables DMA, doesn't get the
  completion it waits for, tears down and retries in a tight loop forever.

## Fix #1 (DONE): 0x40000000 uncached memory alias — `src/physical.rs`

The 6.5 driver polls the VINO descriptor/status ring through an **uncached
alias** of low physical memory at `0x40000000`: it reads `0x48621400` to see
the `0x80000001` STOP markers it wrote at RAM `0x08621400`
(`0x48621cf0 − 0x40000000 = 0x08621cf0`). iris didn't map `0x40000000-`, so
those reads hit `CpuBusErrorDevice`, returned `0xFFFFFFFF`, and flooded the log
with `MC: CPU Error at 48621cf0` (~160k lines). 5.3's driver polled the cached
addresses directly so this never surfaced.

Fix: `alias_phys()` in physical.rs strips bit 30 for addresses in
`0x40000000-0x7FFFFFFF` before the device-map dispatch, so they resolve to the
real RAM/device. Result: the MC error flood is **gone** (0 errors). This is a
correct, standalone fix worth keeping regardless of the capture work.

## Client delivery is blocked at the videod level (ruled out client API)

With the alias fix, the kernel VINO driver captures continuously (~30 fps:
`channel A DMA enabled` grows by ~300 in 5 s) and the camera data reaches RAM.
But **no VL client ever receives a frame**. Tried, all fail identically:
`vlGetNextValid` poll, `vlGetLatestValid` poll, and a `vlSelectEvents` +
`vlNextEvent` loop (which blocks forever — `vlPendingEvents` doesn't exist in
this libvl). Tried source/drain node `0` and `VL_ANY` (the latter negotiates
768×576 PAL vs 640×480 NTSC — note the standard ambiguity). videod captures for
itself but never completes a *client* transfer, so vinograb gets zero frames and
zero events.

Hypotheses **tested and DISPROVEN** as the delivery blocker:
- **Interleave/descriptor geometry (STOP-after-one-field).** Disabling the
  per-row interleave skip in `dma_emit_dword` + the stride pad in
  `render_and_pump` (so a field writes contiguously, consuming half the
  descriptors like MAME, STOP after the frame) did **not** unblock delivery —
  vinograb still timed out. So the geometry only affects image *quality*, not
  whether videod delivers. (Reverted; the 5.3 geometry is unchanged.)
- **Video standard mismatch.** Default is NTSC and the camera feeds NTSC
  (640×486); node-0 capture is 640×480 NTSC and still didn't deliver. (`VL_ANY`
  negotiates 768×576 PAL but that's a separate VL-default quirk, not the cause.)
- **Client API.** poll `vlGetNextValid`, poll `vlGetLatestValid`, and
  `vlSelectEvents`+`vlNextEvent` all fail identically; node 0 and `VL_ANY` both
  fail.

**Decisive control test:** the STOCK `/usr/sbin/vidtomem` — the exact tool the
5.3 campaign confirms works — **also hangs on 6.5** (no output, no file, had to
^C it), identically to the custom `vinograb`. So the custom client was NOT the
bug; the VL capture→client path is broken on 6.5 regardless of client.

So the blocker is below the VL client — in the videod/kernel-VL/emulator
interaction on 6.5: the kernel VINO driver captures and the DESC/EOF interrupts
fire, but no frame ever reaches a VL client. It is NOT the interleave geometry
(disproven), NOT the video standard (disproven), NOT client code (disproven by
the vidtomem control test). Root cause below the client is UNDIAGNOSED — likely
needs comparing 5.3 (works) vs 6.5 (hangs) videod behaviour at the
register/ioctl level, since 5.3 capture works through the same iris VINO/CDMC.

## Root cause of the remaining timeout (NOT yet fixed)

The 6.5 driver waits for the **end-of-descriptor (DESC / `ISR_CHA_DESC`)
interrupt**, which fires when DMA consumes a descriptor with the STOP bit. The
driver lays out a long descriptor chain (3 page-ptrs + a JUMP per 16-byte
group, advancing `0x10` per jump) that ends in a region of `0x80000001` STOP
descriptors (seen at `0x08621400`).

iris's `pump_field()` (`src/vino.rs`) **rewinds to a fixed `start_desc_ptr` at
the start of every field and never advances it**, so it re-traverses the same
front of the chain each field and never reaches the STOP descriptors → the DESC
interrupt never fires → the driver never sees completion.

MAME's `vino_device::end_of_field` (`../mame/src/mame/sgi/vino.cpp`) is the
reference: after the **odd** field it does `start_desc_ptr = next_desc_ptr`
(advance), and after the **even** field it rewinds `next_desc` to
`start_desc_ptr` with `page_index = line_size + 8`. So its traversal progresses
frame-by-frame and eventually hits STOP. iris needs the same advance.

JUMP handling itself is fine (iris uses `& 0x3fffffff`, matching MAME — the
apparent "skip" of the `0x...0` slot is what MAME does too).

## Next steps

1. Rework `pump_field`/interlace so the descriptor chain advances like MAME's
   `end_of_field` (advance `start_desc_ptr` after the odd field) and the STOP
   descriptor is reached → raises `ISR_CHA_DESC`. **Regression-test 5.3
   capture** (the current rewind logic was tuned for 5.3 — fixes #10/#11 in the
   companion note).
2. Re-verify the descriptor data-address mask: MAME uses `& 0x3ffff000`
   (page-aligned, drops top 2 bits); iris uses `& 0xFFFF_FFF0`. Equivalent for
   clean page-aligned descriptors but worth aligning.

## Repro harness

- `vinograb.c` (VL one-frame grab), `mempeek.c` (`/dev/mem` physical reader) —
  both in repo root, stream to `/var/tmp` and `cc` on the guest.
- Build iris with `--features chd,camera,lightning,developer` and run with
  `IRIS_DEBUG_LOG=vino,mc` to trace register access + MC errors.
- `/tmp` is wiped on boot — put guest test binaries in `/var/tmp`.
- root's shell is now `/bin/sh` on the klindert disk (POSIX redirects work).

## 2026-05-30 — DESC delivery on 6.5 SOLVED at the kernel/DMA layer; blocker moved up to videod

Two findings this session, one a fix and one a self-inflicted regression now reverted:

1. **Uncached-alias fix (KEEP — `src/physical.rs`).** 6.5's vino driver polls the
   descriptor ring through the uncached `0x4000_0000` alias of RAM. The bus
   dispatch didn't map that alias, so reads/writes to the ring missed, flooding
   `MC: CPU Error at 48621cf0` and capture never engaged. Added `alias_phys()`
   (`if addr & 0xC000_0000 == 0x4000_0000 { addr & !0x4000_0000 }`) at the head of
   all 9 BusDevice dispatch methods. With this, capture engages on 6.5.

2. **DESC now fires on 6.5 with the KNOWN-GOOD (HEAD) descriptor code.** Confirmed
   directly: `INTR_STATUS -> 0x00000005` (CHA_EOF|CHA_DESC) fires ~1435× during a
   vidtomem run. So the earlier hypothesis that 6.5 needed an interlace
   restructure (one DESC per frame, no per-row skip, no rewind) was WRONG — that
   restructure REMOVED the descriptor-cursor advance that lands the cursor on the
   chain's STOP, so DESC stopped firing. Reverted `src/vino.rs` to HEAD. The
   original per-row interleave skip + stride pad is load-bearing: one field emits
   ~150 page-writes but the chain is 300 aligned pages, so each write must advance
   the cursor ~2 descriptor slots (the skip) to reach STOP and raise DESC.

3. **6.5 driver descriptor layout (observed live).** videod ping-pongs TWO
   channel-A buffers, re-arming `A_NEXT_4_DESC` alternately to `0x0861e000` and
   `0x0a8cc000`, with `A_FIELD_COUNTER` alternating 1<->2. chainwalk (unaligned
   0x3fffffff mask): `0x0861e000` = 181 data pages -> STOP@086214f0;
   `0x0a8cc000` = 4 data pages -> STOP@0a8cc2d0 (asymmetric — not yet explained;
   16-byte-aligned walk of 0x0861e000 = 300 pages = full 640x480x4 frame).

4. **Remaining blocker is in userspace (videod/VL), NOT the emulator DMA.**
   Despite ~1435 DESC interrupts, stock `vidtomem` never receives a frame. `par -s`
   on the hung vidtomem caught its exit path:
   `select([3])=1; read(3,..,32)=0; "VL connection to :0.0 broken (explicit kill
   or server shutdown)"; exit(1)` — i.e. **videod closes the VL connection / dies
   instead of ever delivering a frame**. No vino/video errors in /var/adm/SYSLOG
   and no iris-side errors. So the kernel completes transfers (DESC) but videod's
   frame-done -> VL-buffer-valid path never hands a buffer to the client.

**Next investigation (NOT yet done):** trace `videod` itself (par/par -s on the
1257/1258 pair) across a frame to see which /dev/vino ioctl or register poll it is
waiting on after DESC — i.e. what kernel-visible "buffer N complete" signal videod
expects that iris isn't setting (candidate: FIELD_COUNTER pairing, DESC_TABLE_PTR
readback, or a per-buffer done status videod polls). videod is proprietary, so
this is syscall/ioctl-trace driven.

## 2026-05-30 (cont.) — videod dig: full kernel delivery-path map; root cause localized

Goal of this pass: find why videod never delivers a frame to clients on 6.5 even
though iris now fires DESC+EOF interrupts (see previous section). Combined a LIVE
`par` trace of videod with a full disassembly of the 6.5 kernel driver (vino.o).

### Live videod trace (par -s -i -SS, videod launched under par)
videod opens `/dev/vino` (fd 7), does its analog/camera setup ioctls
(`0x7669000b` blocks ~3 s = video-lock wait, then a burst of `0x76690008`), then
settles into its VL server main loop: `select(1024, [5:6:7:10...], 0,0,0)`. The
ready set returned is ALWAYS client sockets (`[6/10:225:230:231:234...]`) —
**fd 7 (`/dev/vino`) is never in the ready set.** So `/dev/vino` never becomes
poll-readable, videod never collects a captured frame, and the client
(vidtomem / vlGetNextValid) blocks forever. The iris VINO register log shows
continuous autonomous activity (≈5500 capture cycles) DURING videod's idle
select loop — i.e. the kernel ISR (`vinoInterrupt`) IS running on every
EOF/DESC; the interrupt path works. So the break is purely "kernel never marks
/dev/vino poll-ready."

### Kernel poll-readiness mechanism (from vino.o disassembly)
- `vinoPoll` (0x6e1c) reports fd readable IFF `*(dev+0xb8) != 0` (pending-events
  word). Only handles poll bits in `0x41`.
- `*(dev+0xb8)` (pending) is set ONLY inside the static `pollwakeup_fn` (0x6ec8),
  and only when `*(dev+0xb4) & mask != 0`, where `*(dev+0xb4)` is the
  select/poll mask videod sets explicitly via the `vinoSetPollSel` ioctl
  (copyin → `sw a3,0xb4(dev)`). pollwakeup_fn then OR-s mask into `*(dev+0xb8)`
  and calls kernel `pollwakeup()` to wake the select.
- `vinoInterrupt` calls pollwakeup_fn on the **EOF** bit (mask 5 for ch A / 0xa
  for ch B). The **DESC** bit instead calls `vinoEOD` (advances the descriptor
  ring; does NOT itself wake). Full frame delivery to userspace rides on the
  **buffer-completion** path: `vinoEOD`→`vinoGetNextBuffer`→`vinoFinishDMA`
  (which calls kernel `wakeup`) and the state-machine static `0x777c`→wrapper
  `0x6fcc`→pollwakeup_fn — with a different mask than the per-field EOF=5.

### Root cause (localized, not yet fixed)
The interlace **buffer/field completion state machine** never declares a frame
complete, so the delivery pollwakeup (the one whose mask matches videod's
`vinoSetPollSel` registration) never fires, so `*(dev+0xb8)` for that event stays
0, so `vinoPoll` never reports fd 7 ready. The state machine spans:
`vinoInterrupt`→per-channel dispatch `0x5e7c`→predicate `0x7530` (scans buffer
descriptor tables for the `0x80020202` STOP/done sentinel; returns 2 ⇒
"frame ready" which makes the dispatcher signal poll) + sub-handlers
`0x7640/0x77c0/0x78d8` + `vinoEOD` (sets channel state bytes 0x132/0x133/0x134=2/
0x135) + `vinoGetNextBuffer` + `vinoFinishDMA`. It is driven by EOF/DESC order,
the per-channel state bytes, and `A_FIELD_COUNTER`.

### Two concrete, testable iris-side suspects (highest confidence first)
1. **Simultaneous EOF+DESC.** iris raises EOF and DESC in the SAME field pump
   (INTR_STATUS jumps 0x00→0x05). Real VINO fires EOF at end of active video and
   DESC later when the chain's STOP is consumed — two distinct ISR entries. The
   state machine is written for sequential EOF-then-DESC; a combined 0x05 likely
   desyncs the field/buffer bytes so completion never latches. Fix idea: emit EOF
   when the field's active rows finish, then raise DESC as a separate interrupt
   when the descriptor cursor reaches STOP.
2. **field_counter reset per DMA-enable.** `start_channel` (vino.rs:463) zeroes
   `field_counter` on every DMA-enable; the 6.5 driver re-arms DMA per field, so
   the counter reads 1 at most DESCs and can't express the even/odd pairing the
   state machine needs. Fix idea: make A_FIELD_COUNTER free-running across re-arms
   (don't reset in start_channel), reflecting true field parity.

Each test = full rebuild + boot + capture (~6 min), so validate the EOF/DESC
sequencing hypothesis first (strongest). Status: kernel delivery path fully
mapped; exact completion condition in the state machine not yet pinned.

## 2026-05-30 (cont. 2) — completion gate fixed register-by-register; final blocker = buffer ping-pong vs interleave rewind

Drove the kernel completion check (vino.o vinoEOD→0x77c0) from the iris side.
0x77c0 declares a buffer done (returns 2 ⇒ poll-wake videod) iff the live
**A_DESC_TABLE_PTR** read-back has moved off the buffer base — specifically
`s1 != buffer_base && s1 != buffer_base+0x10 && s1 != *(conn+0xc)`, where `s1`
is the channel's A_DESC_TABLE_PTR (reg 0x70, low word read at 0x74).

Fixes applied this pass (all in src/vino.rs, on top of the physical.rs alias):
1. **field_counter free-running** (don't zero in start_channel). More
   hardware-faithful; did NOT by itself fix delivery.
2. **A_DESC_TABLE_PTR read returns the live cursor** (`next_desc_ptr`), not the
   driver-written base (`start_desc_ptr`, still stored on write for the rewind).
   The driver writes the buffer base each field and expects the HARDWARE to
   advance the pointer; reading back the static base made 0x77c0 always see
   "still on base" (returned 1). After this it read base+0x10 — still "just
   started" (0x77c0 also treats base+0x10 as not-done).
3. **next_desc_ptr advances across JUMPs** (shift_descriptors JUMP branch now
   does `next_desc_ptr = target+16`). The 6.5 jump-bug chain is ~all JUMPs, so
   without this the cursor stayed frozen at the re-armed base+0x10. After this
   A_DESC_TABLE_PTR reads an advanced **0x0861e794** (real progress).

Result: register now advances, but **still no frame delivered**, because the
read is CONSTANT at 0x0861e794 every cycle. Root of that: the 6.5 driver
**ping-pongs two ring buffers** — A_NEXT_4_DESC alternates 0x0861e000 (buf A) and
0x141b0000 (buf B) per field — but iris's **interleave rewind** (pump_field,
`if interleave && start_desc_ptr != 0 { descriptor_fetch(start_desc_ptr); ... }`)
resets every field's DMA cursor to start_desc_ptr (= the A_DESC_TABLE_PTR the
driver writes, always 0x0861e000 = buf A). So iris fills buf A every field and
NEVER buf B; A_DESC_TABLE_PTR can only ever report a buf-A address, the driver
never sees the pointer reach the buffer it's waiting on, and 0x77c0 never returns
2.

### Precise remaining blocker + next step
iris must fill the buffer the driver re-armed via **A_NEXT_4_DESC**, not always
rewind to start_desc_ptr. The interleave rewind (iris's single-table even/odd
model) conflicts with the driver's per-field NEXT_4_DESC ping-pong. Next step:
make pump_field honor the live re-arm — walk from the re-armed next_desc_ptr
(track the last NEXT_4_DESC base separately from start_desc_ptr) and apply the
even/odd offset within THAT buffer, instead of unconditionally rewinding to
start_desc_ptr. RISK: this touches the interlace path that IRIX 5.3 delivery
depends on (5.3 uses EOF-only, page-stepped NEXT_4_DESC, no DESC completion), so
it must be gated/validated against 5.3. Uncommitted working tree at this point:
physical.rs (alias) + vino.rs (the 3 changes above). 4 build/test cycles done.

## 2026-05-30 (cont. 3) — FUNDAMENTAL MODELING GAP found; stopping after 6 builds

Continued the register-modeling fixes (rewind-to-rearm-base so iris fills the
buffer the driver actually arms; tried 16-byte JUMP alignment). Both confirmed
fd 7 (/dev/vino) STILL never enters videod's select ready-set — no delivery.

The decisive observation: A_DESC_TABLE_PTR (= live next_desc_ptr) freezes at
~descriptor 120 (0x0861e780), while the driver's descriptor chain runs to its
real STOP at ~descriptor 300 (0x086214f0). With unaligned JUMP following iris
hit a FALSE early STOP there (raised a bogus DESC=0x05); with 16-byte-aligned
following it hits NO stop and raises only EOF=0x01. Either way the cursor stops
at ~120 and never reaches the chain's true end.

ROOT MODELING GAP (the real reason 6.5 capture doesn't deliver):
**iris's DMA is pixel-driven, not descriptor-chain-driven.** `render_and_pump`
emits exactly the clipped pixel rectangle (~one field = ~120-150 pages) and then
STOPS, leaving the descriptor cursor partway through the chain. Real VINO walks
the ENTIRE descriptor chain — writing captured data into every page — until it
consumes the STOP descriptor; for an interlaced 640x480 frame that's ~300 pages,
and the cursor naturally lands on STOP, which is exactly what the driver's
vinoEOD completion check (vino.o 0x77c0) keys on (A_DESC_TABLE_PTR having reached
past the buffer / the chain end). Because iris stops at the pixel count instead
of walking to STOP, the completion pointer is always short and the frame is never
declared done → videod never woken.

Fixing this properly = restructuring the VINO DMA loop to be descriptor-chain-
driven (iterate descriptors to STOP, place each field's data at the interleaved
pages, raise DESC when the STOP descriptor is consumed) rather than pixel-driven.
That is a significant rewrite of render_and_pump/pump_field/dma_emit_dword and
touches the IRIX-5.3 interlace path that currently DELIVERS, so it carries real
regression risk and needs a 5.3-gated, carefully-tested implementation. This is
genuine multi-session work, not a one-line fix.

### State of the working tree at stop
- src/physical.rs: uncached-alias fix — SOLID standalone win (makes 6.5 capture
  engage + DESC fire; no 5.3 regression expected as it's a pure bus-alias fix).
  RECOMMEND COMMITTING THIS ALONE.
- src/vino.rs: register-modeling improvements (field_counter free-run;
  A_DESC_TABLE_PTR read = live next_desc_ptr; next_desc_ptr advances across JUMPs;
  interleave rewind targets the re-armed A_NEXT_4_DESC base). All more
  hardware-faithful and necessary for the eventual fix, but they do NOT by
  themselves deliver a frame and they touch the 5.3 interlace path (UNTESTED on
  5.3). Keep as documented WIP or revert before committing physical.rs.
- jump-align (0x3FFF_FFF0) was tried and REVERTED (it removed DESC entirely).

Net result of the whole campaign: 6.5 IndyCam capture now ENGAGES and the kernel
DMA/interrupt path works (alias fix); the remaining blocker is the pixel-driven
-vs-descriptor-chain-driven DMA model, precisely localized and documented above.

## 2026-05-30 (cont. 4) — descriptor-DMA model now CORRECT (DESC+STOP); delivery still gated on kernel software state

Got the live descriptor chain via a guest-side chaindump (/usr/tmp/chaindump,
reads /dev/mem). Definitive structure of the 6.5 capture chain at 0x0861e000:
**300 linear DATA pages** (sequential frame-buffer pages, NOT interlace-encoded)
**+ 120 jump-bug JUMPs** (one at the end of most 4-descriptor groups, encoded
target carries a +4 low-bit offset), ending with a final **JUMP -> STOP at
0x086214f0** (word 0x80000001). So interlace placement is entirely iris's
line_size-skip job; the chain is a plain linear 640x480 buffer.

Two grounded fixes from this:
1. **16-byte JUMP alignment** (`& 0x3FFF_FFF0`) — required; the intermediate
   jump-bug JUMPs carry +4 offsets and must be followed aligned or the walk
   desyncs.
2. **drain_to_stop** at end of pump_field — after the pixel pump fills the DATA
   pages it stops one descriptor short of the trailing JUMP->STOP; iris now
   walks the remaining descriptors (follow JUMPs / advance past DATA) to consume
   the STOP and raise DESC, as real VINO does.

RESULT: INTR shows 0x05 (DESC fires) AND A_DESC_TABLE_PTR now reads **0x08621500**
— i.e. past the real STOP (0x086214f0 + 0x10). The descriptor-DMA/interrupt model
is now correct end to end at the hardware level.

BUT videod STILL never delivers: a fresh par trace shows fd 7 (/dev/vino) never
enters videod's select ready-set, even with DESC firing and the cursor past STOP.
So the remaining gate is NOT in the VINO register/DMA model — it is in kernel
driver SOFTWARE state that the register interface can't reach:
 - the completion check vino.o 0x77c0 also compares the live ptr against
   *(conn+0xc) (the driver's tracked pointer) and keys on the buffer index
   *(conn+0x104) / count *(conn+0x10c) and the per-channel state byte 0x133;
 - the frame-ready pollwakeup uses the mask videod registered via the
   vinoSetPollSel ioctl (*(dev+0xb4)); the per-field EOF pollwakeup uses mask 5
   and may simply not match videod's registered mask;
 - delivery may require vinoFinishDMA (kernel wakeup + pollwakeup wrapper 0x6fcc)
   to run, which depends on the buffer-queue state machine, not just DESC.

NEXT TECHNIQUE (different from register tracing): inspect the live kernel conn/dev
structs from the guest (/dev/kmem or a small driver-aware probe) to read
*(conn+0xc), *(conn+0x104/0x10c), byte 0x133, and *(dev+0xb4) during a capture,
to see exactly which comparison/state blocks the wakeup. That's a kmem-inspection
task, not a register-model task.

STATE: physical.rs alias (solid) + vino.rs descriptor-DMA model fixes
(field_counter free-run, A_DESC_TABLE_PTR live cursor, 16-byte jump alignment,
per-field rearm rewind, drain-to-STOP). All hardware-faithful and bring the model
to correct DESC+STOP behaviour; UNTESTED on 5.3. 8 build/test cycles this session.

## 2026-05-30 (cont. 5) — KMEM INSPECTION: exact delivery gate found (and my recent changes are counterproductive)

Used icrash on the live guest (it resolves the loaded vino module's symbols).
Anchors: nm vino_going -> 0xc00f0cfc (.data+0x45c, so .data base 0xc00f08a0);
vino_board (.bss+0, 0xc00f1950) holds the device-struct pointer.

Live struct values during a hanging vidtomem grab:
- device = 0x8d2fa9c0 ("vino" magic at +0, reg base 0xa0080000 at +0x20).
- channel-A conn = *(dev+0x38) = 0x93c98780.
- conn fields: +0x04=0x941f4300 (buffer-entry array), +0x0c=**0x0861e000** (buffer
  base = the value the completion check compares against), +0x14=0x8d2fa9c0 (= the
  device struct; this is the poll "dev"), +0x104=0 (buffer index), +0x10c=5 (buffer
  count), byte 0x133=0x01, byte 0x136=0x1e(30), bytes 0x134/0x135=0.
- **poll state on dev: *(dev+0xb0)=0x20 (poll armed), *(dev+0xb4)=0x140 (videod's
  selected event mask), *(dev+0xb8)=0 (pending=0 -> vinoPoll reports NOT ready).**

### The delivery logic (vino.o), decoded against these values
- vinoInterrupt's per-channel dispatch (static 0x5e7c): when byte 0x133!=0 it calls
  the completion check 0x77c0; **if 0x77c0 returns 2 the dispatcher RETURNS EARLY
  and SKIPS the delivery function 0x7640.** 0x77c0 returns 2 when the live
  A_DESC_TABLE_PTR is NOT equal to the buffer base / *(conn+0xc); returns 0/1 when
  it IS at the buffer base. So delivery requires A_DESC_TABLE_PTR == buffer base
  (0x0861e000) at the DESC interrupt.
- **=> My A_DESC_TABLE_PTR live-cursor + drain-to-STOP changes are COUNTERPRODUCTIVE
  for delivery: they make the read 0x08621500 (past STOP) -> 0x77c0 returns 2 ->
  0x7640 is skipped. The original behaviour (A_DESC_TABLE_PTR == base) is what lets
  0x77c0 return 0/1 and reach 0x7640.** (The drain still correctly raises DESC; the
  pointer value is the problem.)
- The actual frame-ready pollwakeup is in 0x7640 at 0x7770: wrapper 0x6fcc with
  mask 0x30. It is reached only when (field_counter - *(conn+0xc0)) > *(conn+0x136)
  (=30) and other field-pairing conditions on *(conn+0x118)&3, byte 0xb8, etc.

### UNRESOLVED puzzle (the real blocker now)
The poll masks don't line up: videod's selected mask *(dev+0xb4)=0x140 (bits 6,8),
but every internal wakeup mask found is disjoint from it — EOF=5 (bits 0,2),
0x7640-delivery=0x30 (bits 4,5), ioctl-path=0x1000/0x2000/0x4000. 0x140 & {5,0x30,
0x1000...} = 0 for all. pollwakeup_fn gates on *(dev+0xb4) & internal_mask, so with
0x140 NOTHING wakes videod. Either *(dev+0xb4)=0x140 is not the field I think it is
(vino_poll/vinoPoll route the poll head via a .bss+0x20 global, not directly via
conn+0x14), or videod re-arms the mask per grab and I sampled a stale value, or the
mask bit-space differs. Resolving this needs: re-read *(dev+0xb4) at the exact
moment vidtomem issues its grab ioctl (correlate with a par trace), and trace
vino_poll/vinoPoll's poll-head (.bss+0x20 = 0xc00f1970) which is what select
actually queries.

### Recommended next steps (next session)
1. REVERT the A_DESC_TABLE_PTR live-cursor + drain-to-STOP changes (they block
   0x7640). Keep DESC firing via the original STOP-in-pump path. Keep physical.rs
   alias. Re-evaluate field_counter free-run (the 0x7640 delta logic uses
   field_counter - *(conn+0xc0); free-run is fine for deltas but verify).
2. Pin the poll-mask space: read .bss+0x20 (0xc00f1970) poll-head + re-read
   *(dev+0xb4) synchronized with a grab, to learn which internal mask actually
   matches what videod selects.
3. Then make iris satisfy the 0x7640 field-delta gate so the wrapper(0x30) fires.

icrash recipe (reusable): `icrash -e 'od <hexaddr> <words>'` reads /dev/kmem;
`icrash -e 'nm <sym>'` resolves module symbols; `icrash -f cmdfile` batches.
chaindump/chainwalk live in /usr/tmp in the guest (extract via dd bs=512 from
/dev/rdsk/dks0d2s0 after iris-ci put, then truncate with bs=1 on the regular file).
