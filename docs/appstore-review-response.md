# App Store review response — IRIS (Submission 2ed07ab1…)

Covers the two issues raised on the 1.0 (20260610.2118) review (June 16, 2026):

1. **Guideline 2.5.1** — private API `_CGSSetWindowBackgroundBlurRadius`.
2. **Guideline 2.4.5(i)** — entitlements without obvious matching functionality
   (`com.apple.security.device.camera`, `com.apple.security.network.server`).

---

## 1. Guideline 2.5.1 — private API (fixed in binary)

The symbol came from the `winit` windowing crate (pulled in by `eframe`), whose
macOS backend calls `CGSSetWindowBackgroundBlurRadius` in `WindowDelegate::set_blur`.
IRIS never requests window blur, but the symbol is linked in regardless and
Apple's static scan flags it.

**Fix:** vendored a patched `winit` (`third_party/winit-0.30.13/`, wired via
`[patch.crates-io]` in the root `Cargo.toml`) that removes the private extern
declarations and makes `set_blur` a no-op. Verified the symbol is gone from the
linked binary:

```
nm -u target/release/iris-gui | grep CGSSetWindowBackgroundBlurRadius   # → no output
```

(`_CGShieldingWindowLevel` remains; it is a public CoreGraphics API and was not
flagged.) See `rules/macos/appstore-private-api.md`.

A new binary is required for this fix, so we also strengthened the two
entitlements below with visible, testable functionality rather than removing
them.

---

## 2. Guideline 2.4.5(i) — entitlement justifications

Both entitlements back real functionality. To make them easy to verify we added
in-app features that exercise each one directly, without needing to boot IRIX.

### `com.apple.security.device.camera`

**What it's for:** IRIS emulates the SGI Indy's **IndyCam** / VINO video-input
hardware. When the user selects the host camera as the video source, IRIS
captures live frames from the Mac's camera (AVFoundation) and feeds them to the
emulated VINO device. The matching `NSCameraUsageDescription` is in `Info.plist`.

**How to test (reviewer steps):**
1. Launch IRIS. In the launcher, open the **Video-In** tab.
2. Click **📷 Test Camera**.
3. macOS shows the camera-permission prompt; allow it.
4. A live preview from the Mac camera appears, with a status line showing the
   capture resolution and a rising frame count. Closing the window releases the
   camera (indicator light turns off).

> Paste-ready reply:
>
> IRIS emulates the SGI Indy IndyCam (VINO video-input) hardware. The camera
> entitlement lets the app capture live video from the Mac's camera and feed it
> to the emulated video-input device. You can verify this directly: open the
> **Video-In** tab and click **Test Camera** — macOS will prompt for camera
> access and the app then shows a live preview from the camera. The matching
> NSCameraUsageDescription is included in Info.plist.

### `com.apple.security.network.server`

**What it's for:** two things —
- The emulator exposes the emulated machine's **serial console** (IRIX ttyd1,
  `127.0.0.1:8881`) and **PROM monitor** on loopback TCP, so a terminal can
  attach to the guest console. The app's own **Serial console…** window connects
  to this server (loopback), which is the visible end-to-end demonstration: the
  emulator *listens* (network.server) and the viewer *connects* (network.client).
- It binds **inbound port-forwards** into the emulated SGI Ethernet (SEEQ 8003)
  when the user configures them on the Networking tab.

(The clean-shutdown "Send IRIX halt" action no longer uses a socket — it now
types at the console in-process — so the server entitlement is only used for the
genuine server features above.)

**How to test (reviewer steps):**
1. Launch IRIS and **Start** a machine (the bundled config boots to the PROM).
2. Open **Machine → Serial console…**.
3. The window shows "● connected to 127.0.0.1:8881" and streams the live guest
   serial console. Typing a line and pressing Enter sends it to the guest.
   This confirms the app's loopback serial **server** is live and accepting a
   connection.

> Paste-ready reply:
>
> IRIS exposes the emulated workstation's serial console and PROM monitor as
> loopback TCP servers (e.g. 127.0.0.1:8881) so a terminal can attach to the
> guest console, and it binds user-configured inbound port-forwards into the
> emulated Ethernet. You can verify this without external tools: Start a machine
> and open **Machine → Serial console…** — the app connects to its own loopback
> serial server and streams the live guest console, and you can type into it.

---

## Summary of binary changes in this resubmission

- Vendored/patched `winit` to drop the private blur API (2.5.1). No window blur
  was ever used.
- Added **Video-In → Test Camera** (live host-camera preview) so the camera
  entitlement is user-visible.
- Added **Machine → Serial console…** (in-app loopback serial viewer) so the
  network.server entitlement is user-visible.
- Moved "Send IRIX halt" to an in-process console path (no longer opens a
  loopback socket).
