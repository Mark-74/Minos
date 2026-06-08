// Live "recent blocks" panel on the dashboard. Reuses the same server-side
// filtered /log/ws feed as the full log page (no filters here — show all),
// prepending compact rows and capping the panel at 10. Rows are built with
// textContent (never innerHTML) since reason/sample come from traffic.
(() => {
  const tbody = document.getElementById("recent-tbody");
  if (!tbody) return;
  const table = document.getElementById("recent-table");
  const empty = document.getElementById("recent-empty");
  const CAP = 10;

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
    tr.appendChild(cell(e.kind, true));
    tr.appendChild(cell(e.reason, false));
    tr.appendChild(cell(e.sample_short, true));
    return tr;
  }

  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const ws = new WebSocket(proto + "//" + location.host + "/log/ws");
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
    while (tbody.children.length > CAP) {
      tbody.removeChild(tbody.lastChild);
    }
  };
})();
