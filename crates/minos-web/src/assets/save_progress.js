// Progressive enhancement for the service Save button: instead of a plain
// POST (which still works with JS off), open a WebSocket to /save-progress so
// the operator watches `uv pip install` output live and learns the outcome
// without losing the page. Lines are inserted with textContent (never
// innerHTML) since install output is not trusted markup.
(() => {
  const form = document.getElementById("save-form");
  if (!form) return;
  const service = form.dataset.service;
  const panel = document.getElementById("save-progress");
  const log = document.getElementById("save-log");

  function append(text) {
    log.textContent += text + "\n";
    log.scrollTop = log.scrollHeight;
  }

  form.addEventListener("submit", (ev) => {
    ev.preventDefault();
    const button = form.querySelector("button");
    if (button) button.disabled = true;
    if (panel) panel.hidden = false;
    if (log) log.textContent = "";

    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const url = proto + "//" + location.host +
      "/services/" + encodeURIComponent(service) + "/save-progress";
    const ws = new WebSocket(url);

    ws.onmessage = (ev) => {
      let msg;
      try {
        msg = JSON.parse(ev.data);
      } catch (_) {
        return;
      }
      if (typeof msg.line === "string") {
        append(msg.line);
      } else if (msg.status === "ok") {
        append("✓ saved as version " + msg.version);
        location.href = "/services/" + encodeURIComponent(service);
      } else if (msg.status === "error") {
        append("✗ error: " + msg.message);
        if (button) button.disabled = false;
      }
    };

    ws.onerror = () => {
      append("✗ connection error");
      if (button) button.disabled = false;
    };
  });
})();
