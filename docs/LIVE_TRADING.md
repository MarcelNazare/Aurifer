# Live Trading

The highest-stakes document in this project. Read it entirely before
setting `IG_BASE_URL` to the live endpoint.

---

## 1. Read this first

**Most users who reach this step lose money.**

The backtest that passed was probably overfit. The demo that matched
probably got lucky. The strategy that "worked" in research often fails
live for reasons no backtest captures: worse fills, wider spreads during
volatility, worse adverse selection, and the psychological weight of
real money on the line.

This is not pessimism. It is the documented base rate. If you proceed,
proceed with the assumption that the strategy will lose more than the
backtest predicted, and that you will discover why only after losses.

**The first trade should be on demo. Stay on demo until the demo works
for 3 months.** Not 3 weeks. Three months. Long enough to see the
strategy through multiple regimes and to catch the bugs that only appear
on live data.

---

## 2. Pre-flight checklist

All must be true before `IG_BASE_URL` points at the live endpoint:

- [ ] **Backtest passed on 3+ years of data.**
      Walk-forward validation cleared (aggregate AUC ≥ 0.54, no fold < 0.50,
      ≥ 2 regimes with edge). If the predictor was auto-disabled, that
      means walk-forward failed — respect it, don't trade the predictor.

- [ ] **Backtest passed on a second calendar year.**
      The first year being profitable is not enough. Year 2 must also pass.
      If year 2 failed, the strategy does not have a tradeable edge.

- [ ] **Demo account has traded for 3+ months.**
      Compare demo results against the backtest over the same period. If
      demo loses money where backtest was profitable, the backtest is
      wrong — the market changed, or the backtest has a bug, or the fill
      model was too optimistic.

- [ ] **Realized adverse gap ≤ 1.5 × modeled slippage.**
      Printed at the end of every run. If it's higher, the backtest's
      cost model is optimistic — raise `Config::slippage` and re-run
      before going live.

- [ ] **You understand every parameter in `Config`.**
      Not "I read the docs." You can explain why each default is what it
      is, and what happens if you change it. `docs/STRATEGY.md` is the
      reference.

- [ ] **You have a manual kill switch.**
      See §7. You know how to close positions via the IG app or web
      interface if the program stops responding.

- [ ] **You can afford to lose the entire account balance.**
      Not "it would hurt." Not "I would learn something." The full balance,
      gone, and you are still financially intact. If not, do not trade live.

If any box is unchecked, stop. Do not go to §3.

---

## 3. IG setup

### 3.1 Create an IG account

1. Go to `https://www.ig.com/` and click "Create live account."
   **Demo accounts can be created first** at `https://www.ig.com/en/demo-account`,
   and you can generate API keys for demo.
2. Complete KYC — ID verification, address, tax residency.
3. Complete the appropriateness questionnaire (required for CFD providers).
4. Wait for verification. Typically 1–3 days.

### 3.2 Generate an API key

Once verified:

1. Log in to your IG account.
2. **My IG → Settings → API Keys → Generate New Key.**
3. You will receive an API key. Save it — you cannot retrieve it later.
4. Note your **IG username**, **password**, and **account ID** (visible in
   the account dropdown on IG's web interface).

### 3.3 Configure `.env`

```dotenv
IG_USERNAME=your_ig_username
IG_PASSWORD=your_ig_password
IG_API_KEY=your_api_key
IG_ACCOUNT_ID=your_account_id

# Demo (default)
IG_BASE_URL=https://demo-api.ig.com/gateway/deal

IG_GOLD_EPIC=CS.D.CFDGOLD.CFDGC.IP
IG_ORDER_SIZE=1.0
```

### 3.4 Confirm demo mode

Before starting the live loop, verify the endpoint:

```bash
grep IG_BASE_URL .env
# must contain "demo" until you're past §2 pre-flight
```

Then run:

```bash
aurifer live --confirm-live
```

The startup line will print:

```
▶ IG live loop (DEMO) — epic CS.D.CFDGOLD.CFDGC.IP
```

If it says `🔴 LIVE MONEY`, stop immediately and check `.env`.

### 3.5 Find your gold epic

IG assigns epics per account type. The default
`CS.D.CFDGOLD.CFDGC.IP` is the 100-oz spot gold CFD on most accounts, but
your entity or account type may have a different one.

**Before your first trade:**

1. Log in to IG web.
2. Search for "gold" in the market search.
3. Note the epic of the instrument you want to trade.
4. Put it in `IG_GOLD_EPIC` in `.env`.

Or from code:

```rust
let results = broker.search("gold").await?;
for (epic, name) in results { println!("{epic}  {name}"); }
```

The epic in `.env` must match exactly, or the bot will silently trade
nothing (IG rejects unknown epics, and `broker.rs` returns the error to
the live loop, which logs it but does not halt — a silent failure mode
you must check for on startup).

---

## 4. The opt-in gate

Real money requires **two** explicit conditions:

1. `IG_BASE_URL` in `.env` must point at the live endpoint
   (`https://api.ig.com/gateway/deal`, no "demo" substring).
2. `--confirm-live` flag on the command line.

**Both** are required. A typo in `.env` that accidentally points at live
will not trade without `--confirm-live`; and `--confirm-live` alone with
`IG_BASE_URL` still on demo does not trade real money.

**To test the gate itself without risking money:**

```bash
# This will refuse to start:
aurifer live
# → Error: Live mode requires --confirm-live. This places REAL orders.

# This will trade demo (IG_BASE_URL has "demo"):
aurifer live --confirm-live
# → ▶ IG live loop (DEMO) — epic CS.D.CFDGOLD.CFDGC.IP
```

Both messages are correct. If you see `LIVE MONEY` when you expected
`DEMO`, stop and check `.env`.

---

## 5. The startup sequence

### 5.1 What the program prints

On a healthy start:

```
✅ IG connected — epic CS.D.CFDGOLD.CFDGC.IP
▶ IG live loop (DEMO) — epic CS.D.CFDGOLD.CFDGC.IP
Primed with 200 bars
▶ Polling IG snapshot every 2s. Ctrl-C to stop.
```

Then silence, punctuated by:
- `✅ LONG SQUEEZE 1.00 @ 2034.50 — deal ref XXXX` when a trade opens
- `snapshot error: ...` if the IG API hiccups (transient, self-recovers)

### 5.2 What each line means

- **`✅ IG connected`** — login succeeded. Credentials are valid.
- **`epic CS.D.CFDGOLD.CFDGC.IP`** — which instrument is being traded.
  **Verify this matches what you expect.**
- **`▶ IG live loop (DEMO)`** — demo vs live. Verify before every run.
- **`Primed with 200 bars`** — the warm-up succeeded. Indicators are
  computed. If this fails, the bot has no signal to work from.
- **`Polling IG snapshot every 2s`** — the live loop is running.
  Bars are built from 2-second snapshots; a new bar closes every 60 s.

### 5.3 What an unhealthy run looks like

Signs to abort immediately:

- **`⚠️ IG connection failed`** on startup → the bot is not connected.
  It will exit before trading.
- **`epic unknown`** or `market not found` → `.env` epic is wrong. Nothing
  will trade.
- **`snapshot error` repeated 5+ times in 10 seconds** → IG API is down
  or rate limit exceeded. Stop, wait, retry.
- **`❌ order failed: ...`** → an order was rejected. Read the error.
  Common causes: insufficient margin, market closed, epic disabled for
  your account.
- **Silence for 30+ minutes on a market that's open** → the live loop is
  hung. Ctrl-C, restart.

### 5.4 What a healthy run looks like

- One snapshot fetch every 2 seconds
- A new bar closed every 60 seconds
- Occasionally, a signal fires and an order is placed
- Position management (SL/TP) handled automatically by IG after order fill
- No `error` or `warn` lines except occasional transient `snapshot error`

If you see sustained healthy output for a day, the plumbing works. The
strategy's profitability is a separate question — the plumbing working
tells you nothing about whether the strategy is good.

---

## 6. The runbook

### 6.1 Position management

**The program handles SL/TP automatically** once an order fills. IG
maintains the stop and limit orders on their side. If the program
crashes, the SL and TP remain active at IG — you are protected by the
broker's own stop mechanism, not by the program staying alive.

**You should still check daily:**

- Open positions match what the program thinks it opened
- SL and TP are set on each position (visible in IG web/app)
- No orphaned positions (open at IG, but not tracked by `SessionState` —
  this happens if the program crashes between placing an order and
  recording it)

**If you find a discrepancy:** close the position manually via IG, then
restart the program. Do not try to reconcile by having the program "adopt"
the orphan position — the position sizing math will be wrong.

### 6.2 Daily review

Every day, at end of session:

1. **Open positions.** Any position older than `max_seconds_in_trade`
   should have been closed by the time stop. If you see one open for
   hours, the program is misbehaving.
2. **Trades taken.** Did the program trade when you expected? If not, was
   it a signal issue (no signal fired) or an execution issue (order
   rejected)?
3. **Realized PnL.** Compare against what the strategy "should" have done
   on today's bars. Large divergence means something is off.

### 6.3 Weekly review

1. **Equity curve.** Rising, falling, flat? Compare against the backtest
   window covering the same dates.
2. **Trade count.** Wildly more or fewer trades than the backtest window
   covering the same dates → regime has changed, or the bot is
   malfunctioning.
3. **Slippage.** Realized fills vs. `entry` price recorded by
   `SessionState`. If the gap is growing, the market is getting less
   liquid, or IG's spreads are widening — consider raising
   `Config::slippage` in the backtest and re-evaluating.

### 6.4 Monthly review

1. **Rerun the backtest** on the past 6 months. Compare walk-forward
   results to the last backtest. If the model now fails validation, the
   regime has shifted — consider stopping live trading.
2. **Retrain the predictor** on the latest 6 months. The current model
   was trained on data that ends before the last 3 months — if those 3
   months were a new regime, the model is stale.
3. **Review the SAMPLE SIZE** of your live trades. If you have 100+
   live trades and the realized win rate is significantly below the
   backtest's 95% CI lower bound, the backtest was optimistic — stop and
   diagnose.

### 6.5 When to shut down

Stop live trading — permanently or until you diagnose — if any of these
happen:

- **Walk-forward validation fails on fresh data.** Run the backtest on
  the last 6 months; if it fails validation now, the edge is gone.
- **Realized Sharpe CI turns negative.** Not "has a drawdown" — the
  95% CI on the live trades' Sharpe ratio goes below zero.
- **Realized slippage keeps growing.** If the average realized adverse
  gap is > 2× the modeled slippage for two weeks straight, the
  backtest is far too optimistic.
- **You're watching the program more than it's working.** If you check
  the app every 15 minutes and feel anxiety about the PnL, you are not
  trading — you are gambling with extra steps. Stop.
- **A code change is pending.** Never leave a bot running while you edit
  `broker.rs`. Kill the process, make the change, rebuild, restart.

---

## 7. Kill switch

### 7.1 Normal shutdown

```bash
Ctrl-C
```

The live loop is `while let ... { }` — Ctrl-C aborts it immediately. Open
positions are **not** closed automatically. You must close them via IG.

**Before you Ctrl-C:** check for open positions. If any exist, close them
via IG web/app first, then Ctrl-C.

### 7.2 Emergency close

If the program is unresponsive and a position is open:

1. **Open IG web interface** (`https://www.ig.com/`) or the IG mobile app.
2. **Find the open position.**
3. **Close it** using the "Close" button.

Do not try to close via the program. If it is unresponsive, it cannot
place orders reliably.

### 7.3 Network drop mid-trade

If the network drops while a position is open:

- **The position remains open at IG.** SL and TP are still active because
  IG holds them, not the local process.
- The program will try to reconnect when the network returns.
- On reconnect, the local `SessionState` will be **out of sync** with
  IG's actual positions — the bot thinks it opened at price X, but the
  actual fill may have been at price Y, or the position may have been
  closed by SL/TP while the program was offline.

**After a network drop:**

1. Close all positions manually via IG.
2. Restart the program.
3. Do not let the bot "catch up" — it cannot reconcile state it never saw.

### 7.4 Process crash mid-trade

Same as §7.3. The bot has no way to know what IG did while it was dead.
Manual close, restart, re-verify.

---

## 8. What to do when something goes wrong

### 8.1 Stream disconnected

```
snapshot error: ...
```

The polling loop prints this and continues. IG's API has occasional
hiccups; a single error is transient.

**If errors repeat 5+ times in 10 seconds:** stop the program, check IG
status page, wait 30 minutes, restart.

**If errors repeat for hours:** the epic may have been disabled, or your
API key may have been revoked. Check the IG dashboard.

### 8.2 Order rejected

```
❌ order failed: ...
```

The error message from IG will say why. Common cases:

| Error | Cause | Fix |
|---|---|---|
| `MARKET_CLOSED` | Market is closed (weekend, holiday) | Wait, or check `trade_weekends` in Config |
| `INSUFFICIENT_FUNDS` | Account balance below margin requirement | Reduce `IG_ORDER_SIZE` or deposit |
| `MARKET_NOT_BORROWABLE` | Can't short this instrument right now | Close the program, do not trade until resolved |
| `EPIC_DISABLED` | The epic is not tradeable on your account | Search for gold again, update `IG_GOLD_EPIC` |
| `RATE_LIMIT` | Too many API calls | Raise the polling interval (currently 2s) |

**Do not retry a rejected order immediately.** IG logs each attempt; a
loop of rejections may trigger a temporary block. Diagnose first.

### 8.3 Fills worse than expected

If `realized_adverse_gap` (printed at backtest end, and tracked in the
live loop) is consistently > 1.5 × `Config::slippage`:

- The market is less liquid than the backtest assumed
- IG's spreads on gold have widened
- Your position size is too large relative to available liquidity

**Action:** raise `Config::slippage` in the backtest, re-run, and see if
the strategy still looks tradeable. If it doesn't, stop live trading.

### 8.4 Equity drawdown exceeds daily limit

`SessionState` halts trading for the day at `daily_loss_limit` (2% by
default). Existing positions are still managed (SL/TP still fires).

**The halt is automatic.** Do not override it. Do not restart to
"reset" the halt — the daily loss is real, and re-enabling trading
the same day means the strategy has not learned anything from the loss.

Wait until the next UTC midnight. The halt lifts automatically.

---

## 9. Retraining workflow

The predictor is trained once at program startup, on data loaded from
IG. In practice:

### 9.1 When to retrain

- **Weekly:** retrain on the last 6 months. If the new model fails
  walk-forward, disable the predictor.
- **After a regime shift:** if the live PnL diverges sharply from the
  backtest on the same period, retrain on the recent data.
- **After a code change to feature extraction:** always retrain. Never
  mix features from two versions of the code.

### 9.2 How to retrain

The program retrains on every startup. There is no separate "retrain"
command. To retrain on fresh data:

```bash
# Pull the last 1000 bars from IG (about 16 hours)
aurifer backtest --start 2024-01-01 --end 2024-01-02

# The walk-forward validation uses those bars. If it passes, the model
# is fresh. If it fails, the predictor is disabled.
```

**For longer retraining windows:**

IG caps single requests at ~1000 bars. To retrain on 6 months of 1-minute
data (~130,000 bars), you need to fetch bars in chunks and concatenate
them — the current code does not do this. Add a chunked fetcher in
`data.rs` when you need it:

```rust
async fn fetch_chunked(broker: &IgBroker, days: i64) -> Result<Vec<Bar>> {
    // fetch day by day, concatenate, sort by ts
}
```

Until then, retraining is limited to the last ~16 hours of 1-minute data.
This is enough for daily recalibration but not for retraining on a
different regime.

### 9.3 Swapping the model without stopping

Not supported. Stop the program, restart with fresh data, accept the
gap in live coverage. There is no "hot-swap" path, and trying to build
one adds risk without benefit.

---

## 10. When to shut down

Live trading is a process, not a state. The conditions to stop are the
same as the conditions to not start:

- **Walk-forward validation fails on fresh data** → the edge is gone
- **Live PnL diverges from backtest** → the backtest was wrong, or the
  regime changed
- **Realized slippage is 2×+ modeled** → the cost model was optimistic;
  the strategy loses after real costs
- **Sharpe CI on live trades includes negatives** → not enough evidence
  of an edge
- **You are anxious about the PnL** → you are gambling, not trading

**The correct response to any of these is not "adjust parameters."** It
is to stop, return to `docs/VALIDATION.md`, and ask whether the strategy
ever had a real edge. Most of the time, the answer is no.

That is not a failure. It is the honest outcome of research. The failures
worth worrying about are the ones that look like success because no one
checked.