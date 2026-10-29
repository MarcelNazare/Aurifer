let ws, chart;
const $ = (id) => document.getElementById(id);
const fmt = (n, d = 2) => (n == null || isNaN(n)) ? "—" : Number(n).toFixed(d);
const fmtPnl = (n) => (n == null || isNaN(n)) ? "—" : `${n >= 0 ? "+" : ""}$${n.toFixed(2)}`;
const esc = (s) => String(s).replace(/[&<>"']/g, c => ({ "&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;" }[c]));

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/ws`);
  ws.onopen = () => setStatus("CONNECTED", "running");
  ws.onclose = () => { setStatus("DISCONNECTED — retrying…", "error"); setTimeout(connect, 1500); };
  ws.onerror = () => setStatus("ERROR", "error");
  ws.onmessage = (e) => { try { update(JSON.parse(e.data)); } catch (err) { console.error(err); } };
}
function send(obj) { if (ws && ws.readyState === 1) ws.send(JSON.stringify(obj)); }
function setStatus(t, c) { const el = $("status"); el.textContent = t; el.className = "status " + (c || ""); }

function update(s) {
  if (s.completed) setStatus("COMPLETE", "complete");
  else if (s.paused) setStatus("PAUSED", "paused");
  else setStatus("RUNNING", "running");
  $("btn-pause").textContent = s.paused ? "Resume" : "Pause";
  $("speed").value = s.speed;
  $("speed-val").textContent = Math.round(s.speed);
  $("warnings").innerHTML = (s.warnings && s.warnings.length)
    ? s.warnings.map(w => `<div class="warning">⚠ ${esc(w)}</div>`).join("") : "";
  $("price").textContent = fmt(s.price);
  $("ohlc").textContent = `O ${fmt(s.ohlc[0])} · H ${fmt(s.ohlc[1])} · L ${fmt(s.ohlc[2])}`;
  $("aar").textContent = `ATR ${fmt(s.atr)} · ADX ${fmt(s.adx,1)} · RSI ${fmt(s.rsi,1)}`;
  const pct = s.total_bars ? (s.bar_index / s.total_bars * 100) : 0;
  $("progress").textContent = `bar ${s.bar_index.toLocaleString()} / ${s.total_bars.toLocaleString()} (${pct.toFixed(1)}%)`;
  const pos = $("position-content");
  if (!s.position) { pos.className = "flat"; pos.innerHTML = "FLAT"; }
  else {
    const p = s.position;
    const sc = p.side === "LONG" ? "long" : "short";
    const uc = p.unrealized >= 0 ? "pos" : "neg";
    pos.className = "";
    pos.innerHTML = `<div class="pos-side ${sc}">${p.side} · ${p.kind}</div>
      <div class="pos-row"><span>Entry</span><b>${fmt(p.entry)}</b></div>
      <div class="pos-row"><span>Units</span><b>${fmt(p.units, 3)}</b></div>
      <div class="pos-row"><span>SL</span><b>${fmt(p.sl)}</b></div>
      <div class="pos-row"><span>TP</span><b>${fmt(p.tp)}</b></div>
      <div class="pos-row"><span>uPnL</span><b class="upnl ${uc}">${fmtPnl(p.unrealized)}</b></div>`;
  }
  const rsi = s.rsi ?? 0, adx = s.adx ?? 0;
  const rf = $("rsi-fill"); rf.style.width = `${Math.min(100, Math.max(0, rsi))}%`;
  rf.className = "gauge-fill" + (rsi > 70 ? " hot" : rsi < 30 ? " cold" : "");
  $("rsi-val").textContent = fmt(rsi, 1);
  const af = $("adx-fill"); af.style.width = `${Math.min(100, Math.max(0, adx / 50 * 100))}%`;
  af.className = "gauge-fill" + (adx >= 25 ? " cold" : "");
  $("adx-val").textContent = fmt(adx, 1);
  const pu = s.prediction ?? 0.5;
  const pf = $("pred-fill"); pf.style.width = `${(pu * 100).toFixed(1)}%`;
  pf.className = "gauge-fill" + (pu > 0.55 ? " cold" : pu < 0.45 ? " hot" : "");
  $("pred-val").textContent = s.prediction != null ? `${(pu*100).toFixed(1)}%` : "—";
  $("trend-row").textContent = s.trend_up ? "Trend: ▲ BULLISH" : "Trend: ▼ BEARISH";
  if (!chart) initChart();
  chart.data.labels = s.equity_history.map((_, i) => i);
  chart.data.datasets[0].data = s.equity_history;
  const prof = s.equity >= s.initial_equity;
  chart.data.datasets[0].borderColor = prof ? "#3fb950" : "#f85149";
  chart.data.datasets[0].backgroundColor = prof ? "rgba(63,185,80,0.1)" : "rgba(248,81,73,0.1)";
  chart.update("none");
  $("stat-equity").textContent = `$${fmt(s.equity)}`;
  const pe = $("stat-pnl"); pe.textContent = fmtPnl(s.total_pnl);
  pe.className = s.total_pnl >= 0 ? "pos" : "neg";
  $("stat-trades").textContent = s.trades_count;
  $("stat-winrate").textContent = `${(s.win_rate * 100).toFixed(1)}%`;
  $("stat-missed").textContent = s.missed_trades;
  document.querySelector("#trades-table tbody").innerHTML = s.recent_trades.map(t => {
    const sc = t.side === "LONG" ? "side-long" : "side-short";
    const pc = t.pnl >= 0 ? "pnl-pos" : "pnl-neg";
    const dt = new Date(t.exit_ts * 1000).toISOString().slice(5, 16).replace("T", " ");
    return `<tr><td>${dt}</td><td class="${sc}">${t.side}</td><td>${t.kind}</td><td>${fmt(t.entry)}</td><td>${fmt(t.exit)}</td><td class="${pc}">${fmtPnl(t.pnl)}</td><td>${t.reason}</td></tr>`;
  }).join("");
}

function initChart() {
  chart = new Chart($("equity-chart").getContext("2d"), {
    type: "line",
    data: { labels: [], datasets: [{ label: "Equity", data: [], borderColor: "#3fb950",
      backgroundColor: "rgba(63,185,80,0.1)", fill: true, pointRadius: 0, borderWidth: 2, tension: 0.15 }] },
    options: { responsive: true, maintainAspectRatio: false, animation: false,
      plugins: { legend: { display: false } },
      scales: { x: { display: false }, y: { ticks: { color: "#8b949e", font: { size: 10 } }, grid: { color: "#30363d" } } } },
  });
}

$("btn-pause").onclick = () => send({ type: "Toggle" });
$("btn-step").onclick = () => send({ type: "Step" });
$("btn-reset").onclick = () => send({ type: "Reset" });
$("speed").oninput = (e) => { const v = parseFloat(e.target.value); $("speed-val").textContent = v; send({ type: "SetSpeed", value: v }); };
connect();