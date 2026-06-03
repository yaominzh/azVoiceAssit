# Target Architecture — Tauri GUI (feat/gui-enhancement, in progress 2026-06-02)

**Status:** In progress. The Rust audio/AEC/barge-in pipeline is **unchanged**. Only the
UI shell changes — egui is replaced by a Tauri v2 WebView running the "Deep Blue Frost"
frontend (transparent floating window, `backdrop-filter: blur(32px)`).

## What changes vs what stays

```mermaid
flowchart TB
  subgraph unchanged["Pipeline — UNCHANGED from shipped main"]
    direction LR
    cpal["cpal capture"] --> aec["AEC (Speex)"] --> vad["Silero VAD"] --> seg["Segmenter"] --> stt["whisper-rs"] --> refine["oMLX refine"] --> tts["Qwen3-TTS"]
  end

  subgraph bridge["Tauri bridge (new in main.rs)"]
    rxui["rx_ui channel\n(from worker)"]
    emit["app_handle.emit()\n'state' | 'turn' | 'clear'"]
    cmds["#[tauri::command]\ntoggle_mic / stop_tts /\nclear_transcript /\nget_settings / apply_settings"]
    txctrl["tx_ctrl channel\n(to worker)"]
    rxui --> emit
    cmds --> txctrl
  end

  subgraph webview["Tauri WebView — static/ HTML/CSS/JS"]
    listen["window.__TAURI__.event.listen\n'state' | 'turn' | 'clear'"]
    invoke["window.__TAURI__.core.invoke\n(button clicks → commands)"]
    frost["Deep Blue Frost UI\nbackdrop-filter: blur(32px)\ntransparent floating window\nalwaysOnTop: true"]
  end

  unchanged -->|"UiEvent via tx_ui"| bridge
  bridge -->|"app.emit()"| webview
  webview -->|"invoke()"| bridge
  bridge -->|"ControlMsg via tx_ctrl"| unchanged
```

## Architecture — Tauri app full picture

```mermaid
flowchart TB
  subgraph tauri_app["Rust binary (Tauri v2)"]
    subgraph pipeline["Audio/AI pipeline — worker.rs (unchanged)"]
      cap["cpal + AEC\n(48kHz → 16kHz)"]
      vad2["Silero VAD\n(ONNX)"]
      stt2["whisper-rs"]
      ref2["refine + history"]
      tts2["TTS client\n(barge-in)"]
      cap --> vad2 --> stt2 --> ref2 --> tts2
    end

    subgraph main_rs["main.rs — Tauri shell"]
      bridge_thread["Bridge thread\nrx_ui → app.emit()"]
      commands["Tauri commands\n(AppBridge { tx_ctrl })"]
      settings2["AppSettings\n(load/save/validate)"]
    end

    pipeline -->|"UiEvent\n(tx_ui → rx_ui)"| bridge_thread
    commands -->|"ControlMsg\n(tx_ctrl → rx_ctrl)"| pipeline
  end

  subgraph webview2["WebView — static/ (HTML/CSS/JS)"]
    ui_frost["☯ Deep Blue Frost\ntransparent floating window\n380×680, alwaysOnTop"]
    transcript2["heard + refined transcript\n+ UTC timestamps\n+ timing badge"]
    settings_ui["Settings panel\n(in-page, slide-up)"]
  end

  bridge_thread -->|"app.emit('state'|'turn'|'clear')"| webview2
  webview2 -->|"invoke('toggle_mic' etc.)"| commands

  ref2 <-->|"HTTP :8002"| omlx2["oMLX (gemma)"]
  tts2 <-->|"HTTP :8123"| qwen3_2["Qwen3-TTS (MLX)"]

  subgraph tauri_config["Tauri config"]
    conf["tauri.conf.json\ntransparent: true\nalwaysOnTop: true\ndecorations: false\nmacOSPrivateApi: true\nwithGlobalTauri: true"]
    cap2["capabilities/default.json\ncore:window:allow-close"]
  end
```

## UI design — Deep Blue Frost

```
┌─ drag region ──────────────────────────────────┐
│ ● [×]              Voice Assistant              │
│  ╔════════════════════════════════════════════╗ │
│  ║  background: rgba(10,12,28, 0.62)          ║ │
│  ║  backdrop-filter: blur(32px)               ║ │
│  ║  ← desktop wallpaper visible through ←    ║ │
│  ║                                            ║ │
│  ║           ☯  (blue glow/pulse)             ║ │
│  ║           LISTENING                        ║ │
│  ║   endpoint ~700ms · stt 88ms · ...         ║ │
│  ║                                            ║ │
│  ║  ┌──────────────────────────────────────┐  ║ │
│  ║  │ heard  18:14:22                      │  ║ │
│  ║  │ to be or not to be                   │  ║ │
│  ║  └──────────────────────────────────────┘  ║ │
│  ║  ┌──────────────────────────────────────┐  ║ │
│  ║  │ refined                              │  ║ │
│  ║  │ To be or not to be.          (blue)  │  ║ │
│  ║  └──────────────────────────────────────┘  ║ │
│  ║                                            ║ │
│  ║  [🎙 Mic] [⏹ Stop] [🗑 Clear]          [⚙] ║ │
│  ╚════════════════════════════════════════════╝ │
└─────────────────────────────────────────────────┘
```

## Key differences from egui version

| Aspect | egui (shipped) | Tauri (target) |
|--------|---------------|----------------|
| UI framework | egui (immediate mode, tool-like) | WebView (full CSS) |
| Transparency | Limited, no `backdrop-filter` | Native macOS blur, frosted glass |
| Animations | Manual painter code | CSS `@keyframes` |
| Settings | In-window egui panel | In-page HTML overlay |
| Window chrome | macOS title bar | Borderless, custom drag region |
| Frontend language | Rust (ui.rs) | HTML/CSS/JS (static/) |
| Pipeline | Unchanged | **Unchanged** |

## Files changed (Tauri migration)

```
rust/
  Cargo.toml          ← remove eframe/egui, add tauri="2"
  build.rs            ← tauri_build::build()
  src/main.rs         ← replace eframe::run_native with Tauri builder + bridge thread
  src/ui.rs           ← DELETED
  src/lib.rs          ← remove pub mod ui
  src/events.rs       ← add Serialize to payload structs
  src/timing.rs       ← add Serialize to TurnTiming
  tauri.conf.json     ← window config (transparent, alwaysOnTop, etc.)
  icons/icon.png      ← required by Tauri build
capabilities/
  default.json        ← Tauri v2 permission model
static/
  index.html          ← Deep Blue Frost layout, drag region, settings panel
  style.css           ← frosted glass, animations, transcript cards
  app.js              ← window.__TAURI__ listen/invoke (all inside DOMContentLoaded)
```
