// Live block-log feed. Opens a WebSocket to the server-filtered /log/ws
// endpoint and prepends each matching entry into the table. The server has
// already applied the filters echoed into MINOS_LOG_WS_URL, so the client
// only renders.
//
// Rows are built with createElement + textContent (never innerHTML) because
// the reason/sample fields are derived from attacker-controlled traffic.
(() => {
  const tbody = document.getElementById("log-tbody");
  if (!tbody || !window.MINOS_LOG_WS_URL) return;

  const table = document.getElementById("log-table");
  const empty = document.getElementById("log-empty");

  function cell(text, code) {
    const td = document.createElement("td");
    if (code) {
      const c = document.createElement("code");
      c.textContent = text;
      td.appendChild(c);
    } else {
      td.textContent = text;
    }
    return td;
  }

  function rowFrom(e) {
    const tr = document.createElement("tr");
    tr.appendChild(cell(String(e.ts), true));
    tr.appendChild(cell(e.service, false));
    tr.appendChild(cell(e.direction, false));
    tr.appendChild(cell(e.kind, true));

    const flag = document.createElement("td");
    if (e.dry_run) {
      const badge = document.createElement("span");
      badge.className = "badge dry";
      badge.textContent = "dry";
      flag.appendChild(badge);
    }
    tr.appendChild(flag);

    tr.appendChild(cell(e.reason, false));
    tr.appendChild(cell(e.sample_short, true));

    const act = document.createElement("td");
    const a = document.createElement("a");
    const params = new URLSearchParams({
      prefill_kind: "regex",
      prefill_pattern: e.sample_short,
    });
    a.href = "/services/" + encodeURIComponent(e.service) +
      "/filters/new?" + params.toString();
    a.textContent = "+ rule";
    act.appendChild(a);
    tr.appendChild(act);
    return tr;
  }

  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const ws = new WebSocket(proto + "//" + location.host + window.MINOS_LOG_WS_URL);
  ws.onmessage = (ev) => {
    let entry;
    try {
      entry = JSON.parse(ev.data);
    } catch (_) {
      return;
    }
    if (table) table.hidden = false;
    if (empty) empty.hidden = true;
    tbody.insertBefore(rowFrom(entry), tbody.firstChild);
    // Cap visible rows to bound memory on a long-lived page.
    while (tbody.children.length > 500) {
      tbody.removeChild(tbody.lastChild);
    }
  };
})();
