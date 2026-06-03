/* Voice Assistant — Tauri frontend (window.__TAURI__ via withGlobalTauri: true) */

const yy     = document.getElementById("taichi");
const status = document.getElementById("status");
const timing = document.getElementById("timing-badge");
const tx     = document.getElementById("transcript");

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

let settingsDraft = {};

// ── Tauri bridge ───────────────────────────────────────────────────────────

const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;

document.addEventListener("DOMContentLoaded", async () => {
    const init = await invoke("get_initial_state");
    updateState(init.state);
    applySettingsDraft(init.settings);

    await listen("state", (e) => updateState(e.payload.value));
    await listen("turn",  (e) => addTurn(e.payload));
    await listen("clear", ()  => clearTranscriptUI());
});

// ── Controls ───────────────────────────────────────────────────────────────

document.getElementById("mic").onclick   = () => invoke("toggle_mic");
document.getElementById("stop").onclick  = () => invoke("stop_tts");
document.getElementById("clear").onclick = () => invoke("clear_transcript");
document.getElementById("btn-close").onclick = () => getCurrentWindow().close();

document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" || (e.metaKey && e.key === "w")) {
        e.preventDefault();
        getCurrentWindow().close();
    }
});

// ── Settings panel ─────────────────────────────────────────────────────────

document.getElementById("settings-btn").onclick = async () => {
    const panel = document.getElementById("settings-panel");
    if (panel.classList.contains("hidden")) {
        const s = await invoke("get_settings");
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
    const defaults = await invoke("get_defaults");
    applySettingsDraft(defaults);
};
document.getElementById("sp-cancel").onclick = () => {
    document.getElementById("settings-panel").classList.add("hidden");
};
