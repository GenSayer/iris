# Mac App Store rejects winit's private SkyLight blur API (`CGSSetWindowBackgroundBlurRadius`)

**Symptom.** App Store review rejects the `iris-gui` binary under **Guideline
2.5.1 (Performance — Software Requirements)**:

> The app uses or references the following non-public or deprecated APIs:
> Contents/MacOS/iris-gui — Symbols: `_CGSSetWindowBackgroundBlurRadius`

**Root cause.** `eframe 0.29` pulls in `winit 0.30` for window creation. winit's
macOS backend (`platform_impl/macos/window_delegate.rs::set_blur`) calls the
private SkyLight APIs `CGSSetWindowBackgroundBlurRadius` /
`CGSMainConnectionID`, declared in `platform_impl/macos/ffi.rs`. The call site
is reached unconditionally during window init (`set_blur(attrs.blur)`), so the
import lands in the linked binary **even though iris-gui never requests blur**
(`egui::ViewportBuilder` leaves `blur = false`). Apple's static binary scan
flags the imported symbol regardless of whether it's called at runtime.

Confirm with:

```
nm -u target/release/iris-gui | grep -i CGSSetWindowBackgroundBlurRadius
```

`U _CGSSetWindowBackgroundBlurRadius` = present (rejected). No output = clean.
(`_CGShieldingWindowLevel` also shows up but is a **public** CoreGraphics API —
Apple does not flag it.)

**Fix.** Vendor a patched winit and override it via `[patch.crates-io]`:

- `third_party/winit-0.30.13/` — copy of the crate with:
  - `set_blur` stubbed to a no-op (no `ffi::CGS…` calls),
  - the two private `extern` declarations removed from `ffi.rs` (and the
    now-unused `NSInteger` / `AnyObject` imports dropped).
- Root `Cargo.toml`: `[patch.crates-io] winit = { path = "third_party/winit-0.30.13" }`.

Only the `0.30.x` requirement (eframe → egui-winit → glutin-winit) matches the
patch. `iris`'s own `winit 0.29` dependency is the keyboard `KeyCode` type only
and creates no window inside `iris-gui`, so its `set_blur` is dead-stripped —
patching just the 0.30 copy removes the symbol entirely (verified with `nm -u`).

**Two-version gotcha.** Cargo allows only one `[patch.crates-io]` entry per
crate name, so you cannot patch both 0.29.15 and 0.30.13. That's fine here —
only the eframe (0.30) window code reaches `set_blur`. If a future change makes
`iris` create a winit-0.29 window inside the GUI process, re-check `nm -u`;
you'd then have to unify on a single winit version before patching.

**When bumping eframe/winit:** re-vendor the matching winit version, re-apply
the two-edit patch, and re-run the `nm -u` check before submitting.

## Upstream status (don't file a new bug — already tracked)

- winit issue **#4205** "_CGSSetWindowBackgroundBlurRadius non-public or
  deprecated API" — open, milestone **winit 0.31.0**.
- winit PR **#4541** "macOS: Feature-gate `CGSSetWindowBackgroundBlurRadius`" —
  open/in-progress. Puts the call behind a `private-apple-apis` Cargo feature
  (off by default → symbol absent unless opted in). Resolves #4205.
- #4574 (dup App Store rejection report) closed as duplicate; #4538 (remove it
  outright) abandoned.

**Migration:** once IRIS moves to a winit (≥0.31) that ships the feature gate —
which only happens after eframe bumps to a winit-0.31 release and we bump eframe
— delete `third_party/winit-0.30.13/` and the `[patch.crates-io]`, and just make
sure the `private-apple-apis` feature stays disabled (and that eframe doesn't
enable it). Re-run the `nm -u` check to confirm.
