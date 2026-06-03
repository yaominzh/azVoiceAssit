/* Voice Assistant — Tauri frontend */

// DOM refs (safe at parse time — HTML is static, elements always present)
const yy     = document.getElementById("taichi");
const status = document.getElementById("status");
const timing = document.getElementById("timing-badge");
const tx     = document.getElementById("transcript");

// ── UI helpers ─────────────────────────────────────────────────────────────

function updateState(value) {
    yy.className = value;
    status.textContent = value.toUpperCase();
}

function addTurn(payload) {
    const div = document.createElement("div");
    div.className = "turn";
    div.innerHTML = `
      <div class="turn-card">
        <div class="card-label">heard  ${payload.timestamp}</div>
        <div class="card-heard">${esc(payload.heard)}</div>
      </div>
      <div class="turn-card">
        <div class="card-label">refined</div>
        <div class="card-refined">${esc(payload.refined)}</div>
      </div>`;
    tx.appendChild(div);
    tx.scrollTop = tx.scrollHeight;
    timing.textContent =
        `endpoint ~${payload.endpoint_ms}ms · stt ${payload.stt_ms}ms · refine ${payload.refine_ms}ms · reply-start +${payload.reply_start_ms}ms`;
    if (tx.children.length > 100) tx.removeChild(tx.firstChild);
}

function clearTranscriptUI() {
    tx.innerHTML = "";
    timing.textContent = "";
}

function esc(s) {
    return s.replace(/[&<>]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));
}

let settingsDraft = {};

function applySettingsDraft(s) {
    settingsDraft = { ...s };
    document.getElementById("sp-prompt").value = s.system_prompt ?? "";
    const sm = s.silence_ms ?? 700;
    document.getElementById("sp-silence").value = sm;
    document.getElementById("sp-silence-val").textContent = sm;
    const th = Math.round((s.speech_threshold ?? 0.5) * 100);
    document.getElementById("sp-thresh").value = th;
    document.getElementById("sp-thresh-val").textContent = (th / 100).toFixed(2);
    const ht = s.history_turns ?? 0;
    document.getElementById("sp-turns").value = ht;
    document.getElementById("sp-turns-val").textContent = ht;
}

// ── Tauri init with retry ──────────────────────────────────────────────────
// window.__TAURI__ is injected by Tauri AFTER the page loads.
// We retry for up to 3 seconds; show an error state if it never arrives.

async function initApp() {
    const { listen } = window.__TAURI__.event;
    const { invoke } = window.__TAURI__.core;
    const appWin   = window.__TAURI__.window.getCurrentWindow();

    // Load initial Rust state
    const init = await invoke("get_initial_state");
    updateState(init.state);
    applySettingsDraft(init.settings);

    // Pipeline events from Rust bridge thread
    await listen("state", (e) => updateState(e.payload.value));
    await listen("turn",  (e) => addTurn(e.payload));
    await listen("clear", ()  => clearTranscriptUI());

    // Controls
    document.getElementById("mic").onclick   = () => invoke("toggle_mic");
    document.getElementById("stop").onclick  = () => invoke("stop_tts");
    document.getElementById("clear").onclick = () => invoke("clear_transcript");
    document.getElementById("btn-close").onclick = () => appWin.close();

    // Drag via startDragging (more reliable than data-tauri-drag-region in v2)
    document.getElementById("titlebar").addEventListener("mousedown", (e) => {
        if (!e.target.closest(".window-controls")) appWin.startDragging();
    });

    // Keyboard shortcuts
    document.addEventListener("keydown", (e) => {
        if (e.key === "Escape" || (e.metaKey && e.key === "w")) {
            e.preventDefault();
            appWin.close();
        }
    });

    // Settings panel
    document.getElementById("settings-btn").onclick = async () => {
        const panel = document.getElementById("settings-panel");
        if (panel.classList.contains("hidden")) {
            const s = await invoke("get_settings").catch(() => settingsDraft);
            applySettingsDraft(s);
            panel.classList.remove("hidden");
        } else {
            panel.classList.add("hidden");
        }
    };

    document.getElementById("sp-silence").oninput = (e) => {
        settingsDraft.silence_ms = Number(e.target.value);
        document.getElementById("sp-silence-val").textContent = e.target.value;
    };
    document.getElementById("sp-thresh").oninput = (e) => {
        settingsDraft.speech_threshold = Number(e.target.value) / 100;
        document.getElementById("sp-thresh-val").textContent = (Number(e.target.value) / 100).toFixed(2);
    };
    document.getElementById("sp-turns").oninput = (e) => {
        settingsDraft.history_turns = Number(e.target.value);
        document.getElementById("sp-turns-val").textContent = e.target.value;
    };
    document.getElementById("sp-apply").onclick = async () => {
        settingsDraft.system_prompt = document.getElementById("sp-prompt").value;
        await invoke("apply_settings", { s: settingsDraft });
        document.getElementById("settings-panel").classList.add("hidden");
    };
    document.getElementById("sp-defaults").onclick = async () => {
        const d = await invoke("get_defaults").catch(() => ({}));
        applySettingsDraft(d);
    };
    document.getElementById("sp-cancel").onclick = () => {
        document.getElementById("settings-panel").classList.add("hidden");
    };
}

function tryInit(attemptsLeft) {
    if (window.__TAURI__) {
        initApp().catch((e) => {
            console.error("initApp failed:", e);
            status.textContent = "INIT ERROR";
            timing.textContent = String(e);
        });
        return;
    }
    if (attemptsLeft <= 0) {
        status.textContent = "NO TAURI API";
        timing.textContent = "window.__TAURI__ not injected after 3s";
        return;
    }
    setTimeout(() => tryInit(attemptsLeft - 1), 100);
}

// Start retrying once DOM is ready (30 attempts × 100ms = 3 second timeout)
document.addEventListener("DOMContentLoaded", () => tryInit(30));

// ── Visual settings (blur + opacity) — localStorage, no Rust needed ────────

const appEl = document.getElementById("app");

function applyVisualSettings(blurPx, opacityPct) {
    appEl.style.backdropFilter = `blur(${blurPx}px)`;
    appEl.style.webkitBackdropFilter = `blur(${blurPx}px)`;
    appEl.style.setProperty("--bg-opacity", (opacityPct / 100).toFixed(2));
}

function loadVisualSettings() {
    const blur    = Number(localStorage.getItem("blur_px")    ?? 8);
    const opacity = Number(localStorage.getItem("opacity_pct") ?? 22);
    applyVisualSettings(blur, opacity);
    // Sync sliders if settings panel already rendered
    const bs = document.getElementById("sp-blur");
    const os = document.getElementById("sp-opacity");
    if (bs) { bs.value = blur;    document.getElementById("sp-blur-val").textContent = blur; }
    if (os) { os.value = opacity; document.getElementById("sp-opacity-val").textContent = opacity; }
}

document.addEventListener("DOMContentLoaded", () => {
    loadVisualSettings();

    document.getElementById("sp-blur").oninput = (e) => {
        const v = Number(e.target.value);
        document.getElementById("sp-blur-val").textContent = v;
        applyVisualSettings(v, Number(document.getElementById("sp-opacity").value));
        localStorage.setItem("blur_px", v);
    };

    document.getElementById("sp-opacity").oninput = (e) => {
        const v = Number(e.target.value);
        document.getElementById("sp-opacity-val").textContent = v;
        applyVisualSettings(Number(document.getElementById("sp-blur").value), v);
        localStorage.setItem("opacity_pct", v);
    };
});
