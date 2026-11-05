let ws, chart;
const $ = (id) => document.getElementById(id);

const fmt = (n, d = 2) =>
  (n == null || isNaN(n)) ? "—" : Number(n).toFixed(d);

const fmtPnl = (n) =>
  (n == null || isNaN(n)) ? "—" : `${n >= 0 ? "+" : ""}$${n.toFixed(2)}`;

const esc = (s) =>
  String(s).replace(/[&<>"']/g, c =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/ws`);

  ws.onopen  = () => setStatus("CONNECTED", "running");
  ws.onclose = () => { setStatus("DISCONNECTED — retrying…", "error"); setTimeout(connect, 1500); };
  ws.onerror = () => setStatus("ERROR", "error");
  ws.onmessage = (e) => {
    try { update(JSON.parse(e.data)); }
    catch (err) { console.error("bad snapshot", err); }
  };
}

function send(obj) {
  if (ws && ws.readyState === 1) ws.send(JSON.stringify(obj));
}

function setStatus(text, cls) {
  const el = $("status");
  el.textContent = text;
  el.className = "status " + (cls || "");
}

function update(s) {
  if (s.completed) setStatus("COMPLETE", "complete");
  else if (s.paused) setStatus("PAUSED", "paused");
  else setStatus("RUNNING", "running");

  $("btn-pause").textContent = s.paused ? "Resume" : "Pause";
  $("speed").value = s.speed;
  $("speed-val").textContent = Math.round(s.speed);

  $("warnings").innerHTML = (s.warnings && s.warnings.length)
    ? s.warnings.map(w => `<div class="warning">⚠ ${esc(w)}</div>`).join("")
    : "";

  $("price").textContent = fmt(s.price);
  $("ohlc").textContent =
    `O ${fmt(s.ohlc[0])} · H ${fmt(s.ohlc[1])} · L ${fmt(s.ohlc[2])}`;
  $("aar").textContent =
    `ATR ${fmt(s.atr)} · ADX ${fmt(s.adx,1)} · RSI ${fmt(s.rsi,1)}`;
  const pct = s.total_bars ? (s.bar_index / s.total_bars * 100) : 0;
  $("progress").textContent =
    `bar ${s.bar_index.toLocaleString()} / ${s.total_bars.toLocaleString()} (${pct.toFixed(1)}%)`;

  const pos = $("position-content");
  if (!s.position) {
    pos.className = "flat";
    pos.innerHTML = "FLAT";
  } else {
    const p = s.position;
    const sc = p.side === "LONG" ? "long" : "short";
    const uc = p.unrealized >= 0 ? "pos" : "neg";
    pos.className = "";
    pos.innerHTML = `
      <div class="pos-side ${sc}">${p.side} · ${p.kind}</div>
      <div class="pos-row"><span>Entry</span><b>${fmt(p.entry)}</b></div>
      <div class="pos-row"><span>Units</span><b>${fmt(p.units, 3)}</b></div>
      <div class="pos-row"><span>SL</span><b>${fmt(p.sl)}</b></div>
      <div class="pos-row"><span>TP</span><b>${fmt(p.tp)}</b></div>
      <div class="pos-row"><span>uPnL</span><b class="upnl ${uc}">${fmtPnl(p.unrealized)}</b></div>
    `;
  }

  const rsi = s.rsi ?? 0;
  const adx = s.adx ?? 0;

  const rf = $("rsi-fill");
  rf.style.width = `${Math.min(100, Math.max(0, rsi))}%`;
  rf.className = "gauge-fill" + (rsi > 70 ? " hot" : rsi < 30 ? " cold" : "");
  $("rsi-val").textContent = fmt(rsi, 1);

  const af = $("adx-fill");
  af.style.width = `${Math.min(100, Math.max(0, adx / 50 * 100))}%`;
  af.className = "gauge-fill" + (adx >= 25 ? " cold" : "");
  $("adx-val").textContent = fmt(adx, 1);

  const pu = s.prediction ?? 0.5;
  const pf = $("pred-fill");
  pf.style.width = `${(pu * 100).toFixed(1)}%`;
  pf.className = "gauge-fill" + (pu > 0.55 ? " cold" : pu < 0.45 ? " hot" : "");
  $("pred-val").textContent = s.prediction != null ? `${(pu*100).toFixed(1)}%` : "—";

  $("trend-row").textContent =
    s.trend_up ? "Trend: ▲ BULLISH" : "Trend: ▼ BEARISH";

  if (!chart) initChart();
  chart.data.labels = s.equity_history.map((_, i) => i);
  chart.data.datasets[0].data = s.equity_history;
  const profitable = s.equity >= s.initial_equity;
  chart.data.datasets[0].borderColor = profitable ? "#3fb950" : "#f85149";
  chart.data.datasets[0].backgroundColor =
    profitable ? "rgba(63,185,80,0.1)" : "rgba(248,81,73,0.1)";
  chart.update("none");

  $("stat-equity").textContent = `$${fmt(s.equity)}`;
  const pnlEl = $("stat-pnl");
  pnlEl.textContent = fmtPnl(s.total_pnl);
  pnlEl.className = s.total_pnl >= 0 ? "pos" : "neg";
  $("stat-trades").textContent  = s.trades_count;
  $("stat-winrate").textContent = `${(s.win_rate * 100).toFixed(1)}%`;
  $("stat-missed").textContent  = s.missed_trades;

  document.querySelector("#trades-table tbody").innerHTML =
    s.recent_trades.map(t => {
      const sc = t.side === "LONG" ? "side-long" : "side-short";
      const pc = t.pnl >= 0 ? "pnl-pos" : "pnl-neg";
      const dt = new Date(t.exit_ts * 1000)
        .toISOString().slice(5, 16).replace("T", " ");
      return `<tr>
        <td>${dt}</td>
        <td class="${sc}">${t.side}</td>
        <td>${t.kind}</td>
        <td>${fmt(t.entry)}</td>
        <td>${fmt(t.exit)}</td>
        <td class="${pc}">${fmtPnl(t.pnl)}</td>
        <td>${t.reason}</td>
      </tr>`;
    }).join("");

  if (s.forecast) {
    $("forecast-content").style.display = "";
    $("forecast-unavailable").style.display = "none";

    const f = s.forecast;
    $("forecast-meta").textContent =
      `next ${f.horizon} ${f.resolution} bar${f.horizon === 1 ? "" : "s"}`;

    const pUpEl = $("forecast-p-up");
    pUpEl.textContent = `${(f.p_up * 100).toFixed(1)}%`;
    pUpEl.className = f.p_up >= 0.55 ? "up" : f.p_up <= 0.45 ? "down" : "flat";

    const dirEl = $("forecast-direction");
    dirEl.textContent = f.direction;
    dirEl.className = f.direction === "LIKELY UP"   ? "up"
                    : f.direction === "LIKELY DOWN" ? "down" : "flat";

    $("forecast-confidence").textContent =
      `${f.confidence} (${f.edge_pct.toFixed(1)}%)`;
    $("forecast-as-of").textContent  = f.last_date;
    $("forecast-price").textContent  = `$${f.last_close.toFixed(2)}`;

    const aucEl = $("forecast-auc");
    aucEl.textContent = f.auc.toFixed(3);
    aucEl.className = f.auc >= 0.54 ? "good" : "weak";

    $("forecast-auc-ci").textContent =
      `[${f.auc_ci_lo.toFixed(3)}–${f.auc_ci_hi.toFixed(3)}]`;

    const brierEl = $("forecast-brier");
    brierEl.textContent = f.brier.toFixed(4);
    brierEl.className = f.brier < 0.24 ? "good" : "poor";

    $("forecast-base-rate").textContent = `${(f.base_rate * 100).toFixed(1)}%`;

    const valEl = $("forecast-validation");
    valEl.textContent = f.validated ? "✅ VALIDATED" : "⚠️ UNVALIDATED";
    valEl.className = f.validated ? "ok" : "bad";

    $("forecast-features").textContent = f.top_features.length
      ? f.top_features.map(([n, w]) =>
          `${n} ${w >= 0 ? "+" : ""}${w.toFixed(2)}`).join(", ")
      : "—";
  } else {
    $("forecast-content").style.display = "none";
    $("forecast-unavailable").style.display = "";
    $("forecast-meta").textContent = "";
  }

  updateNews(s);
}

// ── News panel ────────────────────────────────────────────────────

function updateNews(s) {
  const list = s.news_events || [];
  const content = $("news-content");
  const meta = $("news-meta");

  if (!list.length) {
    content.innerHTML = '<div class="news-empty">No news events in this window.</div>';
    meta.textContent = "";
    return;
  }

  const active = list.filter(e => e.active).length;
  const upcoming = list.filter(e => !e.active && e.ts > s.current_ts).length;
  const parts = [];
  if (active) parts.push(`${active} active`);
  if (upcoming) parts.push(`${upcoming} upcoming`);
  meta.textContent = parts.length ? `(${parts.join(" · ")})` : "";

  content.innerHTML = list.map(e => {
    const when = new Date(e.ts * 1000);
    const stamp = when.toISOString().slice(5, 16).replace("T", " ");
    const rel = fmtRelative(s.current_ts, e.ts);
    const cls = e.active
      ? "news-active"
      : e.ts > s.current_ts ? "news-upcoming" : "news-past";
    const tag = e.kind === "FOMC" ? "fomc" : "headline";
    return `<div class="news-item ${cls}">
      <span class="news-tag ${tag}">${esc(e.kind)}</span>
      <span class="news-time">${stamp}</span>
      <span class="news-rel">${rel}</span>
      <span class="news-title">${esc(e.title)}</span>
    </div>`;
  }).join("");
}

function fmtRelative(now, ts) {
  const d = ts - now;
  const abs = Math.abs(d);
  let n, unit;
  if (abs < 60)        { n = abs;                 unit = "s"; }
  else if (abs < 3600) { n = Math.floor(abs / 60); unit = "m"; }
  else if (abs < 86400){ n = Math.floor(abs / 3600); unit = "h"; }
  else                 { n = Math.floor(abs / 86400); unit = "d"; }
  return d >= 0 ? `in ${n}${unit}` : `${n}${unit} ago`;
}

function initChart() {
  chart = new Chart($("equity-chart").getContext("2d"), {
    type: "line",
    data: {
      labels: [],
      datasets: [{
        label: "Equity",
        data: [],
        borderColor: "#3fb950",
        backgroundColor: "rgba(63,185,80,0.1)",
        fill: true,
        pointRadius: 0,
        borderWidth: 2,
        tension: 0.15,
      }],
    },
    options: {
      responsive: true,
      maintainAspectRatio: false,
      animation: false,
      plugins: { legend: { display: false } },
      scales: {
        x: { display: false },
        y: {
          ticks: { color: "#8b949e", font: { size: 10 } },
          grid: { color: "#30363d" },
        },
      },
    },
  });
}

$("btn-pause").onclick = () => send({ type: "Toggle" });
$("btn-step").onclick  = () => send({ type: "Step" });
$("btn-reset").onclick = () => send({ type: "Reset" });
$("speed").oninput = (e) => {
  const v = parseFloat(e.target.value);
  $("speed-val").textContent = v;
  send({ type: "SetSpeed", value: v });
};

connect();