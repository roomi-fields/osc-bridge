// Device catalogue: live filter over devices.json. Runs only on the page
// that holds the .ob-catalogue block.
(function () {
  const root = document.querySelector(".ob-catalogue");
  if (!root) return;
  const pages = root.dataset.pages;
  const state = { q: "", vendor: "", markers: new Set() };
  let data = { entries: [], vendors: [] };

  const esc = s => String(s).replace(/[&<>"']/g, c =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  function mark(text, q) {
    const s = esc(text);
    if (!q) return s;
    const re = new RegExp("(" + q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + ")", "ig");
    return s.replace(re, "<mark>$1</mark>");
  }
  function coverage(c) {
    const bits = [];
    if (c.commands) bits.push(c.commands + " cmd");
    if (c.sysex_params) bits.push(c.sysex_params + " SysEx");
    if (c.cc) {
      const ex = [];
      if (c.cc14) ex.push(c.cc14 + " 14-bit");
      if (c.nrpn) ex.push(c.nrpn + " NRPN");
      bits.push(c.cc + " CC" + (ex.length ? " (" + ex.join(", ") + ")" : ""));
    }
    if (c.triggers) bits.push(c.triggers + " drum sounds");
    if (c.replies) bits.push(c.replies + " reply");
    return bits.join(" · ") || "—";
  }
  function pageUrl(path) {
    return pages + path.replace(/^devices\//, "").replace(/\.json$/, "/");
  }
  function render() {
    const q = state.q.trim().toLowerCase();
    const rows = data.entries.filter(e => {
      if (state.vendor && e.vendor !== state.vendor) return false;
      if (state.markers.size && !state.markers.has(e.marker)) return false;
      if (!q) return true;
      return (e.vendor + " " + e.name + " " + (e.authors || []).join(" ")).toLowerCase().includes(q);
    });
    document.getElementById("ob-stats").textContent =
      `${rows.length} of ${data.entries.length} drivers · ${data.vendors.length} vendors`;
    document.getElementById("ob-rows").innerHTML = rows.map(e => `
      <tr>
        <td>${e.marker}</td>
        <td class="ob-vendor">${mark(e.vendor, q)}</td>
        <td><a href="${pageUrl(e.path)}">${mark(e.name, q)}</a></td>
        <td class="ob-cov">${coverage(e.coverage || {})}</td>
        <td>${e.authors && e.authors.length ? mark(e.authors.join(", "), q) : ""}</td>
      </tr>`).join("") || `<tr><td colspan="5">No match.</td></tr>`;
  }
  fetch(root.dataset.index).then(r => r.json()).then(json => {
    data = json;
    const sel = document.getElementById("ob-vendor");
    for (const v of data.vendors) {
      const o = document.createElement("option");
      o.value = o.textContent = v;
      sel.appendChild(o);
    }
    document.getElementById("ob-q").addEventListener("input", e => { state.q = e.target.value; render(); });
    sel.addEventListener("change", e => { state.vendor = e.target.value; render(); });
    document.querySelectorAll("#ob-markers .ob-chip").forEach(c => c.addEventListener("click", () => {
      const m = c.dataset.m;
      if (state.markers.delete(m)) c.classList.remove("active");
      else { state.markers.add(m); c.classList.add("active"); }
      render();
    }));
    const q = new URLSearchParams(location.search).get("q");
    if (q) { document.getElementById("ob-q").value = q; state.q = q; }
    render();
  });
})();
