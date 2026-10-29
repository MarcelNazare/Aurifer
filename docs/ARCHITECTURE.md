# Architecture

The mental model you need to change Aurifer safely.

---

## 1. The pipeline

```
┌────────────────────────────────────────────────────────────────────┐
│  broker::IgBroker                                                  │
│  ├─ fetch_bars("MINUTE", N)  → Vec<Bar>   (historical)            │
│  └─ snapshot()               → (bid, offer)  (live)                │
└────────────────────────────────┬───────────────────────────────────┘
                                 │
                                 ▼
┌────────────────────────────────────────────────────────────────────┐
│  Phase 1 — warm-up                                                 │
│  SessionState::step() over every historical bar                    │
│  → produces Vec<Eval> (one per bar)                                │
│  → primes RSI, ADX, ATR, EMAs, Bollinger, Keltner, Donchian        │
└────────────────────────────────┬───────────────────────────────────┘
                                 │
                                 ▼
┌────────────────────────────────────────────────────────────────────┐
│  Phase 2 — predictor training (optional)                           │
│  ├─ build_dataset() → features + labels per bar                    │
│  ├─ walk_forward_validate() over 4 expanding folds                 │
│  │    → rejects model if edge doesn't survive regimes              │
│  └─ CalibratedPredictor::train_calibrated() (if validation passes) │
└────────────────────────────────┬───────────────────────────────────┘
                                 │
                                 ▼
┌────────────────────────────────────────────────────────────────────┐
│  Phase 3 — backtest / live loop                                    │
│  SessionState::step() over bars again, now with predictor attached │
│  → produces Trades + equity curve                                  │
└────────────────────────────────┬───────────────────────────────────┘
                                 │
              ┌──────────────────┼──────────────────┐
              ▼                  ▼                  ▼
        ┌──────────┐      ┌──────────┐      ┌──────────────┐
        │   TUI    │      │   Web    │      │   Report     │
        │ ratatui  │      │ axum+ws  │      │ stats::      │
        │          │      │          │      │ analyze()    │
        └──────────┘      └──────────┘      └──────────────┘
```

Everything above is deterministic. Given the same bars and the same
`Config`, you get the same trades, the same equity curve, and the same
statistics.

---

## 2. The three invariants

If you break any of these, the project loses its meaning. They are not
style preferences.

### Invariant 1 — No lookahead

**Features use only bars at or before `t`. Labels use `t + horizon` strictly.**

```rust
// correct
let features = extract_features(&bars[t], &eval[t], &recent_before_t);
let label    = make_label(&bars, t, horizon, mult, atr[t]);  // reads t+horizon

// forbidden
let features = extract_features(&bars[t], &eval[t], &recent_including_t_plus_1);
let label    = (bars[t + horizon].close > bars[t].close) as bool;
// ^ correct only because horizon >= 1; but if you write features that read
//   bars[t+k] for k>0, you have leaked the future.
```

**Why it matters:** backtest results will look great and live results
will look catastrophic. The whole point of `walk_forward_validate` is to
catch overfitting; lookahead defeats it silently.

**If accuracy exceeds 65% on 1-minute gold, suspect a bug before
celebrating.** There is no known feature set on this timeframe with that
much directional edge.

### Invariant 2 — Next-bar-open fills

**Signals are computed at the close of bar `t`. Fills happen at the open
of bar `t+1`, with spread and slippage applied.**

```rust
// SessionState::step(bar_t_plus_1)
if let Some(p) = self.pending_signal.take() {
    self.try_fill_at_open(bar_t_plus_1, p);  // ← fills at bar.open
}
```

Filling at `bars[t].close` is **fantasy**. That price is only known at the
instant the bar closes, and you cannot get an order filled *at* it in
reality. The systematic overstatement is 1–3 ticks per trade — enough to
turn a profitable backtest into a losing live strategy.

### Invariant 3 — Costs are always included

**Every fill goes through `costs::entry_costs` or `costs::exit_costs`.**
No exceptions, not even in tests.

```rust
let costs = costs::entry_costs(&cfg, bar.ts, atr, units, equity);
let entry = match side {
    Long  => open + costs.spread_half + costs.slippage,
    Short => open - costs.spread_half - costs.slippage,
};
```

Modeled costs are a **floor**, not a realistic estimate. If you see a
backtest where every trade profits, the first thing to check is whether
costs were somehow skipped in a code path.

---

## 3. The state machine — `SessionState::step`

This is the heartbeat. Every interface (backtest, TUI, web, live) calls
`step` once per bar. Nothing else.

```
step(bar):

  1. Day reset
     if bar.ts crosses a UTC midnight:
       day_start_equity = equity
       trades_today     = 0
       halted           = false

  2. If halted → return (do nothing until tomorrow)

  3. If a pending signal exists from the previous bar:
       try_fill_at_open(bar, pending_signal)

  4. eval = engine.on_bar(bar)          // compute indicators + signals
     last_eval = Some(eval.clone())
     last_bar  = Some(*bar)

  5. If we hold a position:
       manage_position(bar, &eval)      // SL/TP/trail/partial/time-stop
       return

  6. Risk gate (in this order):
       if trades_today >= max_trades_per_day → return
       if equity < day_start_equity × (1 - daily_loss_limit) → halt, return
       if bar.ts inside a news blackout window → return

  7. If eval.best() fires a signal:
       pending_signal = Some({sig, atr_at_signal, signal_bar_index})
       // NOT a fill — a fill happens in step 3 of the *next* bar
```

**Why the two-bar dance?** Because step 3 of the next bar is where the
next-bar-open fill happens. There is no way to fill within the same bar
without lookahead. If you collapse this into a single `step`, you have
reintroduced the fill-at-close bug.

---

## 4. The strategy engine

`StrategyEngine::on_bar` returns an `Eval` — a snapshot of indicators plus
zero or more candidate signals:

```rust
pub struct Eval {
    pub bar_index:    usize,
    pub atr:          Option<f64>,
    pub adx:          Option<f64>,
    pub rsi:          Option<f64>,
    pub trend_up:     bool,
    pub trend_ready:  bool,    // 5-min EMA(200) has enough bars
    pub squeeze_ok:   bool,    // we just left a BB-inside-KC state
    pub flow:         Option<FlowState>,
    pub signals:      Vec<Signal>,
    pub prediction:   Option<f64>,  // P(up) from ML model, if attached
}
```

`eval.best()` picks the **highest-priority signal** by `StrategyKind::priority`:

| Priority | Family | Why this order |
|---|---|---|
| 0 | Squeeze | Rare, high-conviction |
| 1 | Prediction | ML — often the most informative when validated |
| 2 | Breakout | Trend-following |
| 3 | OrderFlow | Microstructure |
| 4 | RSI | Mean-reversion, lowest conviction |

**Only one signal is acted on per bar.** If squeeze and breakout both fire,
squeeze wins. This is deliberate: stacking signals creates correlated
exposure, and position sizing assumes one trade at a time.

### Confluence gates

Two optional gates can require agreement before a signal is emitted:

- `require_squeeze_gate` — nothing fires unless `squeeze_ok` is true
- `require_orderflow_confirm` — non-orderflow signals need order-flow agreement

Both default to `false`. In testing they reduce trade count by 60–80% and
generally hurt PnL — use sparingly.

---

## 5. The predictor pipeline

The predictor is a **subsystem with its own gating**. If walk-forward
validation fails, the predictor is not attached to the engine and the
backtest proceeds with rule-based families only.

```
warm up indicators over all bars
    │
    ▼
build_dataset(bars, evals, cfg)
    ├─ extract_features(bar_t, eval_t, recent) → [f64; 15]
    ├─ make_label(bars, t, horizon, mult, atr_t)  → Option<bool>
    └─ returns (Vec<Option<features>>, Vec<Option<label>>, Vec<atr>)
    │
    ▼
walk_forward_validate(bars, evals, cfg, 4 folds)
    ├─ 4 expanding folds: train [0..k], test [k..k+1]
    ├─ per-fold AUC, per-regime breakdown
    ├─ aggregate AUC, worst fold AUC, stddev AUC
    └─ WalkForwardResult { folds, aggregate_auc, ..., regime_summary }
    │
    ▼
wf.validate()
    ├─ aggregate AUC ≥ 0.54?
    ├─ worst fold AUC ≥ 0.50?
    ├─ ≥ 2 regimes with AUC > 0.52 and n ≥ 100?
    └─ if any fails → return Ok(None) → predictor disabled
    │
    ▼
CalibratedPredictor::train_calibrated(bars, evals, cfg)
    ├─ 60% train / 15% calibrate / 25% test
    ├─ fit logistic regression on train
    ├─ fit Platt scaling (a, b) on calibrate → probabilities are real
    ├─ evaluate on test → AUC, Brier, log-loss, bootstrap CI
    └─ returns CalibratedPredictor or bails
    │
    ▼
StrategyEngine::with_predictor(cfg, model)
    └─ on_bar adds Prediction signals when P(up) crosses threshold
```

**The predictor's diagnostic output** (AUC, Brier, CI) is printed to stdout
during training. **Read it.** A predictor with AUC 0.52 is barely
distinguishable from random and will hurt more than help once costs are
included.

---

## 6. The three interfaces

All three interfaces drive the **same `SessionState`** with the same
`step(bar)` call. There is no separate logic for TUI, web, or backtest.

### Backtest — headless

```rust
for bar in &bars { state.step(bar); }
state.force_close(bars.last());
print_report(&state);
```

### TUI — interactive

```rust
loop {
    if !paused { advance(n_bars_this_frame); }
    terminal.draw(|f| app.draw(f));   // reads state.last_eval, state.pos, ...
    if key_pressed { mutate paused/speed/reset }
}
```

`advance` calls `state.step()` for each bar. The only difference from the
backtest is that the loop yields to the terminal between batches.

### Web — streaming

```rust
loop {
    tick.tick().await;                                    // 33 ms
    if !paused && !completed {
        for _ in 0..steps {
            state.step(&bars[idx]);
            idx += 1;
            eq_hist.push(state.equity);
        }
    }
    let snapshot = build_snapshot(&state, &bars, idx, &eq_hist, ...);
    state_tx.send(Some(Arc::new(snapshot)));
}
```

The `watch::channel` broadcasts the latest snapshot; each connected
WebSocket client gets it and forwards to the browser. Commands (pause,
step, reset) flow the other way through an `mpsc::channel`.

### Live — REST polling

```rust
loop {
    tick.tick().await;                                    // 2 s
    let (bid, offer) = broker.snapshot().await?;
    let mid = (bid + offer) / 2.0;
    // aggregate ticks into a 1-minute Bar in-memory
    // when the minute rolls over, call state.step(&completed_bar)
    // if state.pending_signal fires, call broker.place_market_order()
}
```

The live loop bypasses the historical bars entirely. It builds bars from
snapshots and feeds them to the same `SessionState::step()`. This means
**the live code path and the backtest code path share the same strategy
logic** — a critical property. If you find a bug in one, it's a bug in
both, and it will be caught by the backtest first.

---

## 7. Where to extend — and what breaks if you do

### Adding a strategy family

**Touches:**
- `src/types.rs` — add a `StrategyKind` variant, update `as_str`, `priority`
- `src/strategies.rs` — add the detection logic in `on_bar`
- `src/predictor.rs` — optional: add features that capture the new family's state

**What breaks if you get it wrong:**
- Wrong priority → the family is silently ignored (a lower-priority signal always loses)
- Feature leakage in the new features → predictor AUC looks great in-sample, collapses in walk-forward
- No corresponding `docs/STRATEGY.md` entry → future you won't know why it exists

**Required:**
- A walk-forward test proving the new family adds edge — a backtest that
  "looked good once" is not a test.

### Adding a new data source

**Touches:**
- `src/data.rs` — add a `fetch_X_bars()` function
- `src/main.rs` — add a branch in the loading logic
- `src/broker.rs` — if the source also provides execution, add a client

**What breaks if you get it wrong:**
- Bars out of order → indicators silently misbehave
- Weekend/holiday gaps handled differently from `IgBroker` → time-based
  stops behave differently between backtest and live
- Volume units differ → order-flow proxy (`FlowState`) produces different
  signals

**Required:**
- Match the `Bar` schema exactly. If the source has no volume, use `1.0`
  and document that the order-flow family will be disabled.
- Sort bars by `ts` before returning.
- Add a `docs/DATA.md` entry describing the source, its coverage, its rate
  limits, and any known gotchas.

### Adding a new predictor

**Touches:**
- `src/predictor.rs` — the `CalibratedPredictor` type and `predict_proba`

**What breaks if you get it wrong:**
- **Any predictor that returns a point estimate instead of a probability
  is disqualified.** The threshold-based trading rule
  (`p ≥ 0.60 → long, p ≤ 0.40 → short`) has no meaning if `p` is not a
  calibrated probability.
- **Any predictor without a walk-forward validation step is disqualified.**
  If your new model has no `walk_forward_validate()` equivalent, it will
  not be attached to the engine, and the backtest will proceed rule-based
  only. This is intentional.

**Required:**
- A walk-forward test at the same or stricter level as the current one.
- Calibration (Platt, isotonic, or equivalent) so that "60%" actually
  means "correct 60% of the time."
- A bootstrap confidence interval on the AUC. If the 95% CI crosses 0.50,
  the model is disabled regardless of point estimate.

### Adding a new metric

**Touches:**
- `src/stats.rs` — add the computation
- `src/main.rs::print_report` — add the display line
- `src/web.rs::Snapshot` — if it should appear in the dashboard

**What breaks if you get it wrong:**
- A metric with no confidence interval is not a metric — it's a number.
  Every reported quantity in this project should have an uncertainty
  estimate. If you cannot produce one, either derive it (bootstrap,
  analytic) or do not print it.

### Adding a live-order confirmation path

The current `create_order` returns a **deal reference** — IG confirms
asynchronously. Adding confirmation means:

**Touches:**
- `src/broker.rs` — add `get_order_confirmation(deal_ref) → DealConfirmation`
- `src/main.rs::run_live_trading` — poll confirmation before updating local state

**What breaks if you get it wrong:**
- Updating `SessionState.pos` before IG confirms → a rejected order leaves
  `state.pos` set, blocking all future entries until the time stop fires
- Not handling partial fills → position size drifts from `units` computed
  by the sizing logic

**Required:**
- The live loop's local state must **lag** the broker until confirmation.
  Optimistic local state is how bots end up with orphaned positions.

---

## 8. What breaks if you touch this — a summary

| You touched | Also update |
|---|---|
| `SessionState::try_fill_at_open` | `realized_slippage_sum`, TUI position display, Web `PositionView`, `stats::analyze` trade schema |
| `SessionState::manage_position` | `ExitReason` variants, TUI trade list, `stats` win/loss classification |
| `StrategyEngine::on_bar` | `Eval` struct, `build_snapshot` in `web.rs`, TUI gauge display |
| `CalibratedPredictor::predict_proba` | `walk_forward_validate`, the diagnostic in `point_predictor.rs` |
| `costs::entry_costs` / `costs::exit_costs` | Every test that asserts PnL — because all PnL shifts |
| `Config` defaults | `README.md` — the tables must match `impl Default` |

The rule: **anything on the left is load-bearing for something on the
right.** If you change one and forget the other, the build may still pass
while the semantics drift.