const yy = document.getElementById("taichi");
const st = document.getElementById("status");
const tr = document.getElementById("transcript");

function escapeHtml(s) {
  return s.replace(/[&<>]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));
}
function addBubble(cls, text) {
  const d = document.createElement("div");
  d.className = "bubble " + cls;
  d.innerHTML = '<div class="who">' + cls + "</div>" + escapeHtml(text);
  tr.appendChild(d);
  tr.scrollTop = tr.scrollHeight;
}

const es = new EventSource("/events");
es.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.type === "state") {
    yy.className = m.value;
    st.textContent = m.value;
  } else if (m.type === "turn") {
    addBubble("heard", m.heard);
    addBubble("refined", m.refined);
  } else if (m.type === "clear") {
    tr.innerHTML = "";
  }
};

const post = (path) => fetch(path, { method: "POST" });
document.getElementById("mic").onclick = () => post("/control/mic");
document.getElementById("clear").onclick = () => post("/control/clear");
document.getElementById("stop").onclick = () => post("/control/stop");
