# IRIS Interrupt Map — SGI Indy (IP24 / Guinness)

All information derived from: IRIX `kern/sys/IP22.h`, `kern/ml/IP22.c`, IOC2 datasheet,
MAME `ioc2.cpp`/`ioc2.h`, Linux `arch/mips/include/asm/sgi/ip22.h`.

---

## IOC2 register addresses (via HPC3, phys base `0x1FBD9800`)

| Register         | Phys offset | IRIX symbol          | Width |
|------------------|-------------|----------------------|-------|
| L0_STAT (ISR)    | `+0x00`     | `LIO_0_ISR_ADDR`     | u8    |
| L0_MASK          | `+0x04`     | `LIO_0_MASK_ADDR`    | u8    |
| L1_STAT (ISR)    | `+0x08`     | `LIO_1_ISR_ADDR`     | u8    |
| L1_MASK          | `+0x0C`     | `LIO_1_MASK_ADDR`    | u8    |
| MAP_STAT (ISR)   | `+0x10`     | `LIO_2_3_ISR_OFFSET` | u8    |
| MAP_MASK0        | `+0x14`     | `LIO_2_MASK_ADDR`    | u8    |
| MAP_MASK1        | `+0x18`     | `LIO_3_MASK_ADDR`    | u8    |
| MAP_POL          | `+0x1C`     | —                    | u8    |

---

## L0_STAT / L0_MASK — Local 0 (→ CPU IP2)

| Bit  | Mask   | IRIX symbol   | VECTOR         | Device / signal              |
|------|--------|---------------|----------------|------------------------------|
| 0    | `0x01` | `LIO_FIFO`    | `VECTOR_GIO0`  | **FIFO_FULL_N** pin (active-low); on Indy also = REX3 GFIFO-full (GIO_INT_0 from GFX slot) |
| 1    | `0x02` | `LIO_SCSI_0`  | `VECTOR_SCSI`  | SCSI channel 0 (`SCSI0INT` pin) |
| 2    | `0x04` | `LIO_SCSI_1`  | `VECTOR_SCSI1` | SCSI channel 1 (`SCSI1INT` pin) |
| 3    | `0x08` | `LIO_ENET`    | `VECTOR_ENET`  | Ethernet (`ENET_INT` pin)    |
| 4    | `0x10` | `LIO_GDMA`    | `VECTOR_GDMA`  | MC DMA done (`MC_DMA_DONE` pin) |
| 5    | `0x20` | `LIO_CENTR`   | —              | Parallel port (`CENTR_INT`?) |
| 6    | `0x40` | `LIO_GIO_1`   | `VECTOR_GIO1`  | **GRX_INT_N** pin (active-low); REX3 GIO_INT_1 from GFX slot (graphics interrupt) |
| 7    | `0x80` | `LIO_LIO2`    | `VECTOR_LCL2`  | **MAP_INT0** output: fires when any `MAP_STAT & MAP_MASK0` bit is set |

---

## L1_STAT / L1_MASK — Local 1 (→ CPU IP3)

| Bit  | Mask   | IRIX symbol     | VECTOR          | Device / signal              |
|------|--------|-----------------|-----------------|------------------------------|
| 0    | `0x01` | `LIO_POWER`     | `VECTOR_POWER`  | Front panel power button     |
| 1    | `0x02` | `LIO_ISDN_HSCX` | `VECTOR_ISDN_HSCX` | ISDN HSCX (IP24 only)     |
| 2    | `0x04` | `LIO_ISDN_ISAC` | `VECTOR_ISDN_ISAC` | ISDN ISAC                 |
| 3    | `0x08` | —               | —               | (unused on IP24)             |
| 4    | `0x10` | `LIO_HPC3`      | `VECTOR_HPCDMA` | HPC3 DMA done (`HPC_DMA_DONE` pin) |
| 5    | `0x20` | `LIO_AC`        | `VECTOR_ACFAIL` | AC fail (`AC_FAIL_N` pin, active-low) |
| 6    | `0x40` | `LIO_VIDEO`     | `VECTOR_VIDEO`  | VINO video (`VIDEO_VSYNC_N` pin) |
| 7    | `0x80` | `LIO_GIO_2`     | `VECTOR_GIO2`   | **VERT_RETRACE_N** pin (active-low); REX3 GIO_INT_2 from GFX slot (vertical retrace) |

> **IP24 note**: datasheet `LOCAL1_N<0>` (pin 31) and `LOCAL1_N<2>` (pin 30) are "general
> purpose, reserved in INT2" — in IRIX they appear as `GP0`/`GP2` but are not used by any
> standard driver on Indy. `EISA_ERROR_N` (pin 15) maps to `LIO_EISA_MASK` but EISA is
> IP22-fullhouse only.

---

## MAP_STAT / MAP_MASK0 / MAP_MASK1 — Mappable interrupts

MAP_STAT contains 8 active-low inputs (`MAP_INT_N<7:6,3:0>` pins, polarity selectable via
MAP_POL). When `MAP_STAT & MAP_MASK0 != 0`, it drives **MAP_INT0** → L0 bit 7 (`LIO_LIO2`).
When `MAP_STAT & MAP_MASK1 != 0`, it drives **MAP_INT1** → L1 bit 3 (unused on IP24).

**IP22 fullhouse** bits 6–7 = `LIO_DRAIN0`/`LIO_DRAIN1` (GFX FIFO not-full feedback).  
**IP24 Indy** bits 6–7 = `LIO_GIO_EXP0`/`LIO_GIO_EXP1` (expansion slot interrupts).

| Bit  | Mask   | IRIX symbol (IP24)  | VECTOR            | Device / signal              |
|------|--------|---------------------|-------------------|------------------------------|
| 0    | `0x01` | —                   | —                 | (unused / MAP_INT_N<0>)      |
| 1    | `0x02` | —                   | —                 | (unused / MAP_INT_N<1>)      |
| 2    | `0x04` | —                   | —                 | (unused / MAP_INT_N<2>)      |
| 3    | `0x08` | —                   | —                 | (unused / MAP_INT_N<3>)      |
| 4    | `0x10` | `LIO_KEYBD_MOUSE`   | `VECTOR_KBDMS`    | Keyboard / mouse (Z8530)     |
| 5    | `0x20` | `LIO_DUART`         | `VECTOR_DUART`    | Serial DUART (Z85C30)        |
| 6    | `0x40` | `LIO_GIO_EXP0`      | `VECTOR_GIOEXP0`  | **GIO expansion slot 0** interrupt (u64 board → `u64_giointr`) |
| 7    | `0x80` | `LIO_GIO_EXP1`      | `VECTOR_GIOEXP1`  | GIO expansion slot 1         |

`VECTOR_GIOEXP0 = 22` → `lcl_id = 22/8 = 2`, `level = 22 & 7 = 6` →
`MAP_MASK0 |= (1 << 6)` at driver init (`setlclvector(VECTOR_GIOEXP0, u64_giointr, ...)`).

---

## Interrupt routing summary

```
Device            → IOC2 pin          → Register bit   → CPU cause
─────────────────────────────────────────────────────────────────────
SCSI0             → SCSI0INT          → L0 bit 1       → IP2
SCSI1             → SCSI1INT          → L0 bit 2       → IP2
Ethernet (SEEQ)   → ENET_INT          → L0 bit 3       → IP2
MC DMA            → MC_DMA_DONE       → L0 bit 4       → IP2
Parallel port     → (HPC3)            → L0 bit 5       → IP2
REX3 GFIFO full   → FIFO_FULL_N       → L0 bit 0       → IP2
REX3 GIO_INT_1    → GRX_INT_N         → L0 bit 6       → IP2
REX3 vert retrace → VERT_RETRACE_N    → L1 bit 7       → IP3
HPC3 DMA          → HPC_DMA_DONE      → L1 bit 4       → IP3
AC fail           → AC_FAIL_N         → L1 bit 5       → IP3
VINO              → VIDEO_VSYNC_N     → L1 bit 6       → IP3
Keyboard/mouse    → MAP_INT_N<4>      → MAP bit 4      → MAP_INT0 → L0 bit 7 → IP2
Serial (DUART)    → MAP_INT_N<5>      → MAP bit 5      → MAP_INT0 → L0 bit 7 → IP2
GIO EXP0 (u64)    → MAP_INT_N<6>      → MAP bit 6      → MAP_INT0 → L0 bit 7 → IP2
GIO EXP1          → MAP_INT_N<7>      → MAP bit 7      → MAP_INT0 → L0 bit 7 → IP2
Power button      → (front panel)     → L1 bit 0       → IP3
```

---

## REX3 / Newport interrupt detail

Newport uses **GIO_SLOT_GFX** which on IP24 maps directly to the `giointr[]` vectors
(no fan-out via EXTIO needed — that is IP22 fullhouse only):

| GIO_INTERRUPT | VECTOR       | IOC2 pin        | Stat register | Bit    | Use              |
|---------------|--------------|-----------------|---------------|--------|------------------|
| GIO_INT_0     | VECTOR_GIO0  | FIFO_FULL_N     | L0_STAT       | bit 0  | GFIFO above threshold |
| GIO_INT_1     | VECTOR_GIO1  | GRX_INT_N       | L0_STAT       | bit 6  | Graphics (newport gfx int) |
| GIO_INT_2     | VECTOR_GIO2  | VERT_RETRACE_N  | L1_STAT       | bit 7  | Vertical retrace  |

`setgiogfxvec(0, ...)` / `setgiovector(0, GIO_SLOT_GFX, giogfx_intr, 0)` registers
the GFIFO handler at `VECTOR_GIO0` → L0 bit 0.

On IP22 fullhouse all three GIO interrupt lines are shared between GFX slot and expansion
slots; the EXTIO register disambiguates which slot fired. On IP24 Indy, GFX slot gets the
three dedicated pins and expansion slots route through MAP instead.

---

## u64 board (N64 dev board) interrupt detail

```
u64_init():
  setgiovector(GIO_INTERRUPT_0, GIO_SLOT_0, u64_giointr, controller)
  → IP24 path: setlclvector(VECTOR_GIOEXP0, u64_giointr, ...)
  → MAP_MASK0 |= (1 << 6)   i.e. LIO_GIO_EXP0 = 0x40
```

Interrupt flow when N64 sends RDB packet:
1. N64 writes to RDB port (GIO address `0x1F400000`-range on Indy bus)
2. u64 board asserts `MAP_INT_N<6>` (active-low)
3. IOC2: `MAP_STAT bit 6` set → `MAP_MASK0 bit 6` enables → `MAP_INT0` asserted
4. `MAP_INT0` → `L0_STAT bit 7` (`LIO_LIO2`) set → `L0_MASK bit 7` enables → **IP2**
5. CPU enters IP2 handler → `lcl0_intr` → dispatches via `lcl0vec_tbl[VECTOR_LCL2+1]`
6. `lcl2_intr` reads `MAP_STAT & MAP_MASK0` → bit 6 set → dispatches `lcl2vec_tbl[7]`
7. `u64_giointr` runs, reads RDB register, dispatches to RDB handler

In IRIS: `IocInterrupt::GioExp0` sets `map_stat |= LIO_GIO_EXP0 (0x40)`.
`update_interrupts()` propagates: if `map_stat & map_mask0 != 0` → sets `l0_stat bit 7`
→ if `l0_mask bit 7` set → fires IP2.

---

## IRIS `IocInterrupt` enum → register mapping

| Variant          | Register  | Bit    | Mask   |
|------------------|-----------|--------|--------|
| `Scsi0`          | L0_STAT   | bit 1  | `0x02` |
| `Scsi1`          | L0_STAT   | bit 2  | `0x04` |
| `Ethernet`       | L0_STAT   | bit 3  | `0x08` |
| `McDma`          | L0_STAT   | bit 4  | `0x10` |
| `FifoFull`       | L0_STAT   | bit 0  | `0x01` |
| `Graphics`       | L0_STAT   | bit 6  | `0x40` |
| `VertRetrace`    | L1_STAT   | bit 7  | `0x80` |
| `HpcDma`         | L1_STAT   | bit 4  | `0x10` |
| `AcFail`         | L1_STAT   | bit 5  | `0x20` |
| `Vino`           | L1_STAT   | bit 6  | `0x40` |
| `Gp0`            | L1_STAT   | bit 0  | `0x01` |
| `Gp2`            | L1_STAT   | bit 2  | `0x04` |
| `GioExp0`        | MAP_STAT  | bit 6  | `0x40` |
| `GioExp1`        | MAP_STAT  | bit 7  | `0x80` |
