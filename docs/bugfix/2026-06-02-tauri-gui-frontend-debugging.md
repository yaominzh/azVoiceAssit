# Bug Post-Mortem: Tauri GUI Frontend — JS Silent Failures

**Date:** 2026-06-02  
**Component:** `feat/gui-enhancement` — Tauri v2 WebView frontend  
**Symptom:** App window opened, ☯ showed, TTS played — but no transcript text, no button
responses, window could not be dragged, × button did not close.

---

## Bug chain (5 sequential issues)

| # | Symptom | Root cause | Fix |
|---|---------|-----------|-----|
| 1 | AEC shadow suppressed mic even when silent | Stale TTS reference frames in AEC cancelling real speech | Pass `None` to processing thread (raw frames to VAD) |
| 2 | Buttons non-functional, transcript empty | `window.__TAURI__` destructured at parse time, before Tauri injected it → throw halts script | Move all `window.__TAURI__` access inside `DOMContentLoaded` |
| 3 | Window could not be dragged | `data-tauri-drag-region` attribute not reliably picked up in Tauri v2 | Replace with `appWin.startDragging()` on `mousedown` |
| 4 | Close button / Esc / Cmd+W silent | `getCurrentWindow().close()` failed without visible error; `core:window:allow-close` not applied | Wrap in try/catch; confirmed capability needed `core:window:allow-start-dragging` too |
| 5 | `event.listen not allowed` INIT ERROR (visible in status) | `capabilities/default.json` was at repo root; Tauri v2 reads capabilities **relative to `tauri.conf.json`** | Move to `rust/capabilities/default.json` |

---

## Issue 1 — AEC suppressing real mic input

**Symptom:** No VAD triggers. AEC shadow logs showed `clean_rms ≈ 0` even during speech (raw_rms = 0.35+).

**Root cause:** The processing thread always ran AEC with `Some(echo_arc)`. After any TTS playback, stale reference frames remained in the AEC queue. When the user spoke afterward, the AEC cancelled their voice against those stale references, outputting near-silence to the VAD.

**Fix:** Pass `None` to `audio::start_processing_thread` so raw mic frames go directly to VAD. The worker still holds `echo_arc` for TTS reference feeding during playback — only the always-on AEC suppression was removed.

```rust
// Before (suppressed user voice):
let _processing = audio::start_processing_thread(rx_raw, tx_processed, Some(echo_arc.clone()));

// After (raw frames to VAD):
let _processing = audio::start_processing_thread(rx_raw, tx_processed, None);
```

---

## Issue 2 — `window.__TAURI__` undefined at parse time

**Symptom:** All buttons non-functional, no transcript, no state updates. Completely silent failure — no console output visible in the terminal.

**Root cause:** The original `app.js` destructured `window.__TAURI__` at the top level:

```js
const { listen } = window.__TAURI__.event;   // line 59 — runs at parse time
const { invoke } = window.__TAURI__.core;
```

Tauri injects `window.__TAURI__` *after* the page is parsed — so at parse time `window.__TAURI__` is `undefined`, `window.__TAURI__.event` throws `TypeError: Cannot read properties of undefined`, and the **entire script halts**. All `onclick` handlers and `listen()` calls were never reached.

**Why silent:** JavaScript module errors in a WebView don't show in the Rust terminal unless explicitly caught and displayed.

**Fix:** Move all `window.__TAURI__` access inside `DOMContentLoaded`, and add a **retry loop** with visible error fallback:

```js
function tryInit(attemptsLeft) {
    if (window.__TAURI__) {
        initApp().catch((e) => {
            status.textContent = "INIT ERROR";
            timing.textContent = String(e);   // error visible in UI
        });
        return;
    }
    if (attemptsLeft <= 0) {
        status.textContent = "NO TAURI API";
        return;
    }
    setTimeout(() => tryInit(attemptsLeft - 1), 100);
}
document.addEventListener("DOMContentLoaded", () => tryInit(30));
```

The error display in the `timing` badge (green monospace text) is what made Issue 5 diagnosable.

---

## Issue 3 — Window not draggable

**Symptom:** Dragging the title bar had no effect.

**Root cause:** `data-tauri-drag-region` is an HTML attribute that Tauri's WebView is supposed to recognize, but in Tauri v2 its reliability depends on the WebView implementation and platform. Child elements (the close button, label span) can intercept pointer events and prevent the drag from registering.

**Fix:** Replace the attribute with explicit JS `startDragging()`:

```js
document.getElementById("titlebar").addEventListener("mousedown", (e) => {
    if (!e.target.closest(".window-controls")) appWin.startDragging();
});
```

Also required adding `core:window:allow-start-dragging` to capabilities.

---

## Issue 4 — Close button / keyboard shortcuts silent

**Symptom:** Clicking × did nothing. Esc/Cmd+W did nothing.

**Root cause:** `getCurrentWindow().close()` was called without error handling. The call may have been silently rejected by the capability system (before `core:window:allow-close` was confirmed applied), or thrown and been swallowed by the async handler.

**Fix:** Wrap in try/catch:

```js
document.getElementById("btn-close").onclick = async () => {
    try { await appWin.close(); } catch(e) { console.error("close failed:", e); }
};
```

Required capability: `core:window:allow-close`.

---

## Issue 5 — `event.listen not allowed` (visible via the Issue 2 fix)

**Symptom:** Status showed **`INIT ERROR`**, timing badge showed:  
`event.listen not allowed. Permissions associated with this command: core:event:allow-listen, core:event:default`

**Root cause:** `capabilities/default.json` was placed at the **repo root** (`/Users/allen/repo/azVoiceAssist/capabilities/`). Tauri v2 resolves the capabilities directory **relative to `tauri.conf.json`**. Our config is at `rust/tauri.conf.json`, so Tauri looked for `rust/capabilities/`. The file at the repo root was silently ignored, and the default empty capability set was used — blocking `event.listen`.

**Fix:** Move the file to the correct location:

```bash
mkdir -p rust/capabilities
mv capabilities/default.json rust/capabilities/default.json
touch rust/build.rs  # force rebuild so tauri-build re-reads capabilities
cargo build
```

Final `rust/capabilities/default.json`:
```json
{
  "permissions": [
    "core:default",
    "core:event:default",
    "core:event:allow-listen",
    "core:event:allow-emit",
    "core:window:default",
    "core:window:allow-close",
    "core:window:allow-start-dragging"
  ]
}
```

---

## General lessons

1. **Make JS failures visible.** A silent exception in a WebView looks identical to "nothing happened." The `status`/`timing` badge error display (Issue 2 fix) turned a 30-minute mystery into a 2-minute diagnosis for Issue 5.

2. **Tauri v2 path resolution is relative to `tauri.conf.json`, not the repo root.** If the config is inside a crate subdirectory (`rust/`), all referenced paths (`frontendDist`, `capabilities/`) must be relative to that subdirectory.

3. **`window.__TAURI__` is injected asynchronously.** Do not access it at parse time. Always access inside `DOMContentLoaded` or with a retry loop.

4. **`data-tauri-drag-region` is fragile.** Use `appWin.startDragging()` on `mousedown` for reliable drag behavior.

5. **All Tauri capabilities must be explicitly listed.** The default capability set does NOT include `event.listen`, `window.close`, or `window.startDragging`. Each must be added to the capabilities JSON.

6. **Changing `capabilities/` requires `cargo build` to take effect** — `tauri_build::build()` bakes them in at compile time.
