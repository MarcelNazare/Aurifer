#![allow(dead_code)]

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path as AxumPath, State,
    },
    response::IntoResponse,
    routing::{delete, get},
    Json, Router,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

use crate::config_store;
use crate::session::SessionState;
use crate::types::*;

const INDEX_HTML: &str = include_str!("../static/index.html");
const SETTINGS_HTML: &str = include_str!("../static/settings.html");
const STYLE_CSS: &str = include_str!("../static/style.css");
const APP_JS: &str = include_str!("../static/app.js");
const SETTINGS_JS: &str = include_str!("../static/settings.js");

#[derive(Clone)]
struct WebState {
    state_tx: watch::Sender<Option<Arc<Snapshot>>>,
    cmd_tx: mpsc::Sender<WsCommand>,
}

#[derive(Serialize, Clone)]
struct Snapshot {
    bar_index: usize,
    total_bars: usize,
    current_ts: i64,
    price: f64,
    ohlc: [f64; 4],
    atr: Option<f64>,
    adx: Option<f64>,
    rsi: Option<f64>,
    prediction: Option<f64>,
    trend_up: bool,
    squeeze_ok: bool,
    position: Option<PositionView>,
    equity: f64,
    initial_equity: f64,
    trades_count: usize,
    win_rate: f64,
    total_pnl: f64,
    missed_trades: usize,
    recent_trades: Vec<TradeView>,
    equity_history: Vec<f64>,
    paused: bool,
    speed: f64,
    completed: bool,
    halted: bool,
    warnings: Vec<String>,
    forecast: Option<ForecastView>,
    news_events: Vec<NewsEventView>,
}

#[derive(Serialize, Clone)]
struct PositionView {
    side: String,
    kind: String,
    entry: f64,
    units: f64,
    sl: f64,
    tp: f64,
    unrealized: f64,
}

#[derive(Serialize, Clone)]
struct TradeView {
    side: String,
    kind: String,
    entry_ts: i64,
    exit_ts: i64,
    entry: f64,
    exit: f64,
    pnl: f64,
    reason: String,
    bars_held: usize,
}

#[derive(Serialize, Clone)]
struct ForecastView {
    horizon: usize,
    resolution: String,
    last_date: String,
    last_close: f64,
    p_up: f64,
    direction: String,
    confidence: String,
    edge_pct: f64,
    auc: f64,
    auc_ci_lo: f64,
    auc_ci_hi: f64,
    brier: f64,
    base_rate: f64,
    validated: bool,
    top_features: Vec<(String, f64)>,
}

impl From<ForecastSummary> for ForecastView {
    fn from(f: ForecastSummary) -> Self {
        Self {
            horizon: f.horizon,
            resolution: f.resolution,
            last_date: f.last_date,
            last_close: f.last_close,
            p_up: f.p_up,
            direction: f.direction,
            confidence: f.confidence,
            edge_pct: f.edge_pct,
            auc: f.auc,
            auc_ci_lo: f.auc_ci_lo,
            auc_ci_hi: f.auc_ci_hi,
            brier: f.brier,
            base_rate: f.base_rate,
            validated: f.validated,
            top_features: f.top_features,
        }
    }
}

#[derive(Serialize, Clone)]
struct NewsEventView {
    ts: i64,
    kind: String,
    title: String,
    active: bool,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum WsCommand {
    Pause,
    Resume,
    Toggle,
    Step,
    Reset,
    SetSpeed { value: f64 },
}

#[allow(clippy::too_many_arguments)]
pub async fn run_web_server(
    bars: Vec<Bar>,
    cfg: Config,
    warnings: Vec<String>,
    predictor: Option<crate::predictor::CalibratedPredictor>,
    forecast: Option<ForecastSummary>,
    news_events: Vec<NewsEvent>,
    open_browser: bool,
) -> Result<()> {
    let (state_tx, _) = watch::channel::<Option<Arc<Snapshot>>>(None);
    let (cmd_tx, cmd_rx) = mpsc::channel::<WsCommand>(32);
    tokio::spawn(sim_loop(
        bars, cfg, warnings, state_tx.clone(), cmd_rx, predictor, forecast, news_events,
    ));

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/settings", get(serve_settings))
        .route("/static/style.css", get(serve_style))
        .route("/static/app.js", get(serve_js))
        .route("/static/settings.js", get(serve_settings_js))
        .route("/ws", get(ws_handler))
        .route(
            "/api/credentials",
            get(api_list_credentials).post(api_set_credential),
        )
        .route("/api/credentials/{key}", delete(api_delete_credential))
        .with_state(WebState { state_tx, cmd_tx });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let url = format!("http://localhost:{port}");
    println!("Aurifer web UI: {url}");
    println!("   Settings:    {url}/settings");

    if open_browser {
        let url = url.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if let Err(e) = webbrowser::open(&url) {
                eprintln!("⚠️ could not open browser: {e}. Open {url} manually.");
            }
        });
    }

    axum::serve(listener, app).await?;
    Ok(())
}

// ── static asset handlers ────────────────────────────────────────

async fn serve_index() -> impl IntoResponse {
    (
        [(axum::http::header::CACHE_CONTROL, "no-store, must-revalidate")],
        axum::response::Html(INDEX_HTML),
    )
}
async fn serve_settings() -> impl IntoResponse {
    (
        [(axum::http::header::CACHE_CONTROL, "no-store, must-revalidate")],
        axum::response::Html(SETTINGS_HTML),
    )
}
async fn serve_style() -> impl IntoResponse {
    ([(axum::http::header::CONTENT_TYPE, "text/css")], STYLE_CSS)
}
async fn serve_js() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        APP_JS,
    )
}
async fn serve_settings_js() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        SETTINGS_JS,
    )
}

// ── credentials API ─────────────────────────────────────────────

async fn api_list_credentials() -> impl IntoResponse {
    match config_store::list_all() {
        Ok(credentials) => Json(serde_json::json!({
            "ok": true,
            "path": config_store::config_path().display().to_string(),
            "credentials": credentials,
        }))
        .into_response(),
        Err(e) => server_error(&e.to_string()),
    }
}

#[derive(Deserialize)]
struct SetCredentialReq {
    key: String,
    value: String,
}

async fn api_set_credential(Json(req): Json<SetCredentialReq>) -> impl IntoResponse {
    if req.key.trim().is_empty() {
        return bad_request("key is required");
    }
    if req.value.is_empty() {
        return bad_request("value is required");
    }
    if !req
        .key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return bad_request("key must contain only A-Z, 0-9, and _");
    }
    match config_store::set_and_apply(&req.key, &req.value) {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => server_error(&e.to_string()),
    }
}

async fn api_delete_credential(AxumPath(key): AxumPath<String>) -> impl IntoResponse {
    match config_store::unset(&key) {
        Ok((removed, _)) => {
            Json(serde_json::json!({ "ok": true, "removed": removed })).into_response()
        }
        Err(e) => server_error(&e.to_string()),
    }
}

fn bad_request(msg: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "ok": false, "error": msg })),
    )
        .into_response()
}

fn server_error(msg: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "ok": false, "error": msg })),
    )
        .into_response()
}

// ── simulation loop / WS handler ────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn sim_loop(
    bars: Vec<Bar>,
    cfg: Config,
    warnings: Vec<String>,
    state_tx: watch::Sender<Option<Arc<Snapshot>>>,
    mut cmd_rx: mpsc::Receiver<WsCommand>,
    predictor: Option<crate::predictor::CalibratedPredictor>,
    forecast: Option<ForecastSummary>,
    news_events: Vec<NewsEvent>,
) {
    let mut state = match predictor {
        Some(p) => SessionState::with_predictor(cfg.clone(), p),
        None => SessionState::new(cfg.clone()),
    };
    let forecast_view: Option<ForecastView> = forecast.map(|f| f.into());
    let news_events: Arc<Vec<NewsEvent>> = Arc::new(news_events);

    let mut idx = 0usize;
    let mut paused = false;
    let mut speed = 30.0_f64;
    let mut eq_hist: Vec<f64> = vec![state.equity];
    let mut accum = 0.0_f64;
    let mut last = Instant::now();
    let mut completed = false;
    let mut tick = tokio::time::interval(Duration::from_millis(33));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = tick.tick() => {
                let now = Instant::now();
                let dt = now.duration_since(last).as_secs_f64();
                last = now;
                if !paused && !completed && idx < bars.len() {
                    accum += dt * speed;
                    let steps = (accum.floor() as usize).min(5000);
                    accum -= steps as f64;
                    for _ in 0..steps {
                        if idx >= bars.len() { break; }
                        state.step(&bars[idx]);
                        idx += 1;
                        eq_hist.push(state.equity);
                    }
                    if idx >= bars.len() {
                        if let Some(l) = bars.last().copied() {
                            state.force_close(&l);
                            eq_hist.push(state.equity);
                        }
                        completed = true;
                        paused = true;
                    }
                }
                let snap = build_snapshot(
                    &state, &bars, idx, &eq_hist, paused, speed, completed,
                    &warnings, forecast_view.clone(), &news_events,
                );
                let _ = state_tx.send(Some(Arc::new(snap)));
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(WsCommand::Pause) => paused = true,
                    Some(WsCommand::Resume) => { if !completed { paused = false; } }
                    Some(WsCommand::Toggle) => { if !completed { paused = !paused; } }
                    Some(WsCommand::Step) => {
                        paused = true;
                        if idx < bars.len() {
                            state.step(&bars[idx]);
                            idx += 1;
                            eq_hist.push(state.equity);
                        }
                    }
                    Some(WsCommand::SetSpeed { value }) => { speed = value.clamp(0.1, 10000.0); }
                    Some(WsCommand::Reset) => {
                        state = SessionState::new(cfg.clone());
                        idx = 0;
                        eq_hist = vec![state.equity];
                        accum = 0.0;
                        completed = false;
                        paused = false;
                    }
                    None => return,
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_snapshot(
    state: &SessionState,
    bars: &[Bar],
    idx: usize,
    hist: &[f64],
    paused: bool,
    speed: f64,
    completed: bool,
    warnings: &[String],
    forecast: Option<ForecastView>,
    news: &[NewsEvent],
) -> Snapshot {
    let bar = bars.get(idx.saturating_sub(1)).copied();
    let price = bar.map(|b| b.close).unwrap_or(0.0);
    let ohlc = bar.map(|b| [b.open, b.high, b.low, b.close]).unwrap_or([0.0; 4]);
    let sim_ts = bar
        .map(|b| b.ts)
        .or_else(|| bars.first().map(|b| b.ts))
        .unwrap_or(0);

    let n = state.trades.len();
    let wins = state.trades.iter().filter(|t| t.pnl > 0.0).count();
    let wr = if n == 0 { 0.0 } else { wins as f64 / n as f64 };
    let pnl: f64 = state.trades.iter().map(|t| t.pnl).sum();

    let recent = state.trades.iter().rev().take(20).map(|t| TradeView {
        side: t.side.as_str().into(),
        kind: t.kind.as_str().into(),
        entry_ts: t.entry_ts,
        exit_ts: t.exit_ts,
        entry: t.entry,
        exit: t.exit,
        pnl: t.pnl,
        reason: t.reason.as_str().into(),
        bars_held: t.bars_held,
    }).collect();

    let pos = state.pos.as_ref().map(|p| PositionView {
        side: p.side.as_str().into(),
        kind: p.kind.as_str().into(),
        entry: p.entry,
        units: p.units,
        sl: p.sl,
        tp: p.tp,
        unrealized: p.unrealized(price),
    });

    let lo = sim_ts - 30 * 60;
    let hi = sim_ts + 24 * 3600;
    let news_views: Vec<NewsEventView> = news
        .iter()
        .filter(|e| e.ts >= lo && e.ts <= hi)
        .take(20)
        .map(|e| {
            let (b_lo, b_hi) = e.blackout();
            NewsEventView {
                ts: e.ts,
                kind: e.kind.as_str().to_string(),
                title: e.title.clone(),
                active: b_lo <= sim_ts && sim_ts <= b_hi,
            }
        })
        .collect();

    Snapshot {
        bar_index: idx,
        total_bars: bars.len(),
        current_ts: sim_ts,
        price,
        ohlc,
        atr: state.last_eval.as_ref().and_then(|e| e.atr),
        adx: state.last_eval.as_ref().and_then(|e| e.adx),
        rsi: state.last_eval.as_ref().and_then(|e| e.rsi),
        prediction: state.last_eval.as_ref().and_then(|e| e.prediction),
        trend_up: state.last_eval.as_ref().map(|e| e.trend_up).unwrap_or(false),
        squeeze_ok: state.last_eval.as_ref().map(|e| e.squeeze_ok).unwrap_or(false),
        position: pos,
        equity: state.equity,
        initial_equity: state.cfg.initial_equity,
        trades_count: n,
        win_rate: wr,
        total_pnl: pnl,
        missed_trades: state.missed_trades,
        recent_trades: recent,
        equity_history: downsample(hist, 300),
        paused,
        speed,
        completed,
        halted: state.halted(),
        warnings: warnings.to_vec(),
        forecast,
        news_events: news_views,
    }
}

fn downsample(v: &[f64], max: usize) -> Vec<f64> {
    if v.len() <= max { return v.to_vec(); }
    let step = (v.len() as f64 / max as f64).ceil() as usize;
    v.iter().step_by(step).copied().collect()
}

async fn ws_handler(ws: WebSocketUpgrade, State(s): State<WebState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, s))
}

async fn handle_socket(socket: WebSocket, state: WebState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.state_tx.subscribe();

    let initial_json = {
        let guard = rx.borrow();
        guard.as_ref().and_then(|s| serde_json::to_string(&**s).ok())
    };
    if let Some(j) = initial_json {
        let _ = sender.send(Message::Text(j.into())).await;
    }

    let mut send_task = tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let json = {
                let guard = rx.borrow();
                guard.as_ref().and_then(|s| serde_json::to_string(&**s).ok())
            };
            if let Some(j) = json {
                if sender.send(Message::Text(j.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    let cmd_tx = state.cmd_tx.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(t) = msg {
                if let Ok(cmd) = serde_json::from_str::<WsCommand>(&t) {
                    if cmd_tx.send(cmd).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }
}