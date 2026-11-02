# Live Trading

**Aurifer does not currently support live trading.**

`aurifer live` returns an error:

```
Error: Live trading is not supported with xaus.com.
       Use IG Markets for execution.
```

xaus.com is a data source, not a broker. The project is currently in
**research mode** — it fetches historical data, trains models, backtests
strategies, and prints forecasts. It does not place orders.

This document explains why, and what the integration path looks like
when you're ready to add live execution.

---

## 1. Why no live trading

Three reasons, in order of importance:

### 1. The predictor does not have a validated edge

The current model on 10 years of daily xaus data:

```
Holdout AUC:   0.474 (95% CI 0.431–0.515)
Brier:         0.2522 (poor)
Walk-forward:  UNVALIDATED (aggregate 0.481, worst fold 0.457)
```

Trading a model with AUC below 0.50 is a losing proposition. Live
trading is for executing a strategy you've already proven works — not
for discovering whether it does.

### 2. xaus.com only provides data

There is no `/api/v1/order` endpoint. xaus is a data vendor. To place
orders you need a broker — IG Markets, Interactive Brokers, OANDA, or a
similar regulated firm. Each has its own API, authentication scheme, and
order schema.

### 3. The project's own discipline forbids it

`aurifer backtest` prints `🚫 DO NOT TRADE LIVE` when a strategy fails
significance checks. `aurifer forecast` prints `⚠️  UNVALIDATED` when the
model fails walk-forward. `aurifer live` refuses to run because the
prerequisite — a validated edge — doesn't exist.

This isn't obstruction. It's the design. The whole point of Aurifer is
to prevent the failure mode where someone wires up a broker account to a
backtest that looked good once, and loses money in a week.

---

## 2. When live trading would make sense

The sequence is not negotiable. All of these must be true:

- [ ] **Predictor validates on 3+ years of data.**
      Walk-forward AUC CI lower bound ≥ 0.54 across the full window.
      Current status: fails (AUC 0.474).

- [ ] **Rule-based strategies show edge.**
      `aurifer backtest --no-predictor` reports `Significant: YES` with
      PnL CI not crossing zero. Current status: untested.

- [ ] **Backtest on a different calendar year.** The first pass might be
      luck. Test on 2022 (downtrend) and 2024 (uptrend) separately.
      Current status: untested.

- [ ] **Demo account has traded for 3+ months.** Compare demo results
      against the backtest over the same period. Current status: no
      broker integration exists.

- [ ] **Realized adverse gap ≤ 1.5 × modeled slippage.** Printed at the
      end of every backtest once a broker is wired in. Current status:
      not applicable (no broker).

- [ ] **You understand every parameter in `Config`.** Not "I read the
      docs." You can explain why each default is what it is, and what
      happens if you change it. See `docs/STRATEGY.md`.

- [ ] **You have a manual kill switch.** Every broker has a web interface
      or mobile app where you can close positions manually. You know
      where it is before you start.

- [ ] **You can afford to lose the entire account balance.** Not "it
      would hurt." The full balance, gone, and you are still financially
      intact.

If any box is unchecked, live trading is not appropriate yet.

---

## 3. The integration path

When you have a validated strategy and decide to add live execution,
here is the shape of the work.

### Add a broker client to `src/broker.rs`

The cleanest structure is a second broker type alongside `XausBroker`:

```rust
pub struct IgBroker { /* ... */ }
pub struct OandaBroker { /* ... */ }
pub struct IbkrBroker { /* ... */ }
```

Each implements:
- `connect() -> Result<Self>` — authenticate
- `place_market_order(side, size, sl, tp) -> Result<String>` — place and
  return the broker's confirmation ID
- `snapshot() -> Result<(f64, f64)>` — live bid/ask
- `close_all() -> Result<()>` — emergency close

xaus stays in place for the historical data path. The two brokers
operate in different worlds:

| Concern | xaus.com | Broker |
|---|---|---|
| Historical bars | ✅ primary | ⚠️ may be limited |
| Live prices | ✅ spot quote | ✅ bid/ask |
| Order placement | ❌ | ✅ |
| Account state | ❌ | ✅ |
| Position tracking | ❌ | ✅ |

### Add a `LiveBroker` trait

```rust
#[async_trait]
pub trait LiveBroker: Send + Sync {
    async fn place_market_order(
        &self, side: Side, size: f64,
        sl: Option<f64>, tp: Option<f64>,
    ) -> Result<String>;
    async fn snapshot(&self) -> Result<(f64, f64)>;
    async fn close_all(&self) -> Result<()>;
}
```

`XausBroker` does not implement this. The IG/OANDA/IBKR clients do.
`run_live_trading` takes `Box<dyn LiveBroker>` and the rest of the loop
is unchanged from the previous version.

### Add two opt-in gates

Real money requires **two** explicit conditions:

1. An environment variable (e.g. `IG_LIVE=1`) that flips the broker
   client from demo to live endpoint.
2. A `--confirm-live` flag on the command line.

**Both** are required. A typo in `.env` that accidentally points at live
will not trade without `--confirm-live`; and `--confirm-live` alone with
the endpoint still on demo does not trade real money.

This is a lesson learned from the previous IG integration. Real
credentials are easy to leak; real money is easy to move. Two
independent gates make accidental live trading unlikely.

### Keep the local state conservative

The live loop's `SessionState` must **lag** the broker:

```rust
// Do NOT do this:
state.pos = Some(Position { ... });
let result = broker.place_market_order(...).await;
// If the order was rejected, state.pos is now wrong.

// Do this:
let result = broker.place_market_order(...).await?;
// Only update state after the broker confirms.
state.pos = Some(Position { ... });
```

Optimistic local state is how bots end up with orphaned positions —
positions open at the broker that the bot doesn't know about, so the
next sizing calculation is wrong.

### Handle partial fills

A 10-unit order might fill as 3 + 4 + 3 at different prices. The
broker's confirmation includes the actual fill price and size. Use
those, not the requested values.

```rust
let fill = broker.place_market_order(...).await?;
let actual_units = fill.filled_units;   // not the requested `units`
let actual_price = fill.avg_fill_price; // not the requested `entry`
```

### Add a kill switch

Every broker has a web interface. Your runbook should include:

1. **Normal shutdown:** Ctrl-C the program. Open positions remain at the
   broker — SL and TP are already set from the entry, so they still fire.
2. **Emergency close:** open the broker's web interface, close the
   position manually.
3. **Network drop mid-trade:** the position remains open at the broker.
   After reconnect, **do not let the bot "catch up"** — close manually,
   restart clean.
4. **Process crash mid-trade:** same as network drop. The bot cannot
   reconcile state it never saw.

See the previous version of this document (preserved in git history)
for the full runbook. The essentials are: verify open positions daily,
compare live PnL to backtest on the same period weekly, and stop if
Sharpe CI turns negative on the live trades.

---

## 4. What not to do

- **Don't run `aurifer live` today.** It errors out. That's the correct
  behavior until the strategy validates.
- **Don't ask "but what if I just try it small?"** The demo is there to
  test infrastructure. Trading on a backtest that failed significance
  checks — even "just a little" — is choosing to lose money slowly.
- **Don't add execution "for convenience" while the predictor fails.**
  The temptation is real. The correct sequence is research → demo →
  live. Not research → live (because you got impatient).

---

## 5. The honest summary

**Aurifer is a research instrument.** It tells you when a strategy has
an edge — and refuses to trade when it doesn't. The current model has no
edge on daily gold. That's not a problem to solve; it's an answer.

The path to live trading is:

1. Find a strategy that passes walk-forward validation.
2. Run it on demo for 3 months.
3. Compare demo to backtest.
4. Only then add the broker client and wire up real execution.

Until step 1 is done, live trading is a distraction from the actual work.

**The "DO NOT TRADE LIVE" verdict is not a bug. It is the point.**