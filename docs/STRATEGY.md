# Strategy

What each of the five families actually does, and why the defaults are
what they are.

**Read this before you change any `Config` value.** Most parameters are
coupled — tuning one moves three others out of equilibrium.

---

## 1. Philosophy

### Why five families?

No single signal reliably produces edge on 1-minute gold. Each family
exploits a different regime:

| Family | Works when | Hurts when |
|---|---|---|
| RSI mean-reversion | Price oscillates inside a range | Trending markets (fades every push) |
| Breakout | Ranges break with momentum | Choppy markets (every breakout is fake) |
| Squeeze release | Volatility compresses then expands | Low-volatility regimes (compression persists) |
| OrderFlow | Bar closes strongly with volume | Thin markets, news bars |
| ML Prediction | Whatever the model has learned | Whatever the model hasn't (regime shifts) |

The families are not redundant. Running only one means long stretches
with no edge; running all five means some family is usually producing
signals — but never more than one at a time (priority ordering ensures
that).

### Why one signal per bar?

Because position sizing assumes one trade at a time. Stacking signals
correlates exposure: if breakout and RSI both fire, they usually point
the same direction, and you take double exposure to the same thesis. The
`priority()` function forces a single winner.

### Why confluence gates exist but default to off

`require_squeeze_gate` and `require_orderflow_confirm` force the families
to agree. In testing they reduce trade count by 60–80% and generally
*hurt* PnL — because they remove too many marginal-but-positive trades.
They exist because some users want "only trade when everything aligns"
behavior. Default: off.

---

## 2. Family 1 — RSI mean-reversion

### Entry rule

**Long:**
- 1-minute RSI(14) crosses **up** through 30 (was <30, now ≥30)
- 5-minute close is above EMA(200)  ← trend filter
- 1-minute ADX(14) ≥ 20           ← motion filter

**Short:**
- 1-minute RSI(14) crosses **down** through 70
- 5-minute close is below EMA(200)
- 1-minute ADX(14) ≥ 20

### Why crosses, not levels

If the rule were "RSI < 30," a sustained oversold reading (RSI sitting at
28 for 20 minutes) would fire the signal on every one of those bars. The
cross fires exactly once per oversold excursion, which matches what you
would actually trade.

### Why the trend filter

Without it, RSI mean-reversion fades every push in a strong trend — the
most expensive failure mode of the strategy. Requiring price above the
5-min EMA(200) for longs means you only fade pullbacks in an uptrend,
which is statistically a different (and better) trade.

### Why the ADX filter

In flat markets, RSI oscillates in a narrow band and produces many weak
signals. ADX ≥ 20 filters out the flattest regimes. Without it, RSI fires
2–3× more often and the extra trades are net negative.

### Known failure modes

- **Trending markets:** even with the EMA(200) filter, a strong trend can
  push through the filter level and then keep going.
- **News spikes:** RSI can hit 30 or 70 and reverse on the same bar,
  producing a signal you cannot actually fill.
- **Session opens:** the first 10 minutes after London open often produce
  RSI whipsaws that trigger both long and short within minutes.

### Which regimes it works in

Best: slow trend + occasional pullbacks. Worst: violent directional moves
and dead-flat ranging.

### Default parameters and why

| Param | Value | Rationale |
|---|---|---|
| `rsi_period` | 14 | Wilder's original; 14 minutes is a reasonable "short-term" |
| `rsi_low` / `rsi_high` | 30 / 70 | Classic thresholds; tighter (25/75) reduces trades without helping PnL |
| `adx_period` | 14 | Standard; ADX is noisy below 14 |
| `adx_min` | 20 | Below this, ADX doesn't distinguish trend from chop |
| `trend_ema_period` | 200 (5-min) | ~16 hours of price — long enough to survive intraday noise |

---

## 3. Family 2 — Donchian breakout

### Entry rule

**Long:** close > highest high of past `donchian_period` bars (excluding current), AND ADX ≥ 20
**Short:** close < lowest low of past `donchian_period` bars, AND ADX ≥ 20

### Why "prior" high, not "including current"

`Donchian::prior()` returns the high/low of the previous N bars, excluding
the current. Including the current bar would mean the current bar's high
is always ≥ itself, so `close > highest_high` can never fire.

### Why close-based, not intrabar

If the rule were "high > prior high," every intrabar wick through the
level would fire. On gold, that's most of them. Requiring the *close* to
be above means only sustained breaks count — roughly 60% fewer signals,
all higher quality.

### Why ADX ≥ 20

Breakouts in low-ADX regimes are 70%+ false. ADX is not a perfect filter,
but it eliminates the worst regime.

### Known failure modes

- **Ranges that don't break:** in true range-bound markets, both long and
  short signals fire repeatedly, each losing a tick or two.
- **News breakouts:** a gap through the prior high can close back below it
  before the next bar — the signal fires, then reverses immediately.
- **Session opens:** the London/NY handover frequently has fake breakouts.

### Default parameters and why

| Param | Value | Rationale |
|---|---|---|
| `donchian_period` | 10 | ~10 minutes; longer windows miss short-term ranges |
| `adx_min` | 20 | Same threshold as RSI |

---

## 4. Family 3 — Squeeze release

### Entry rule

The squeeze is a two-state machine:

**In squeeze** when:
- Bollinger upper < Keltner upper, AND
- Bollinger lower > Keltner lower

This means volatility (Bollinger, σ-based) has contracted below the
"normal" range implied by ATR (Keltner).

**Release** happens when the conditions no longer hold.

**Long signal:** on or within `squeeze_lookback` bars after release, IF:
- close > Bollinger upper, AND
- LinReg momentum of (close − Donchian midline) is positive

**Short signal:** mirror image.

### Why both conditions (BB and KC)?

Bollinger alone compresses with any volatility drop. Keltner uses ATR,
which is smoothed. When BB < KC, both σ and ATR are compressed relative
to each other — a stronger signal than σ alone.

### Why momentum confirmation?

A squeeze release can be spurious (one bar of noise). Requiring LinReg
momentum in the same direction filters out the noise releases. Roughly
40% of releases are filtered, and the remaining 60% have meaningfully
higher win rate.

### Known failure modes

- **Squeeze persists for hours:** in low-volatility regimes (Asian
  session), the squeeze may never release. Positions opened on hope lose
  to noise.
- **Double release:** a squeeze can release, then compress again within
  a few minutes, producing two signals in the same direction. The
  lookback window (3 bars) mitigates this but doesn't eliminate it.

### Default parameters and why

| Param | Value | Rationale |
|---|---|---|
| `bb_period` / `bb_mult` | 20 / 2.0 | Standard; 2σ captures ~95% of recent range |
| `kc_period` / `kc_mult` | 20 / 1.5 | Standard; 1.5 × ATR matches BB scale |
| `squeeze_mom_period` | 12 | LinReg on 12 bars responds to 15–30 min moves |
| `squeeze_lookback` | 3 | Long enough to catch the release bar, short enough to avoid stale signals |

---

## 5. Family 4 — OrderFlow proxy

**⚠️ This is a proxy, not footprint data.** Real stacked-imbalance requires
a paid feed (Databento, CQG). The proxy uses bar close location and volume
ratio as a stand-in.

### Entry rule

For each bar, compute:
- `delta_ratio = 2 × (close − low) / range − 1` — ranging from −1 (closed
  at low) to +1 (closed at high)
- `vol_ratio = volume / SMA(volume, 20)` — ranging from 0 (dead bar) to N

**Strong bar:** `|delta_ratio| ≥ min_delta_ratio` (0.60) AND `vol_ratio ≥ min_vol_ratio` (1.30)

**Stacked counter:** increment if same direction as previous strong bar,
reset otherwise.

**Signal:** when `|stacked| ≥ required_stacked` (2), fire in that direction.

### Why stack count?

A single strong bar is common. Two consecutive strong bars in the same
direction is a microtrend — a real pattern, if a weak one. The default of
2 is the minimum for the proxy to be meaningful.

### Known failure modes

- **Thin markets:** in low-volume periods, `vol_ratio` triggers on noise.
- **News bars:** the close location is extreme (near 1 or −1) but the
  follow-through is zero — you buy the top of a spike.
- **Trend days:** every bar closes near its high in a strong uptrend, so
  the proxy fires constantly and takes every pullback in the face.

### Default parameters and why

| Param | Value | Rationale |
|---|---|---|
| `min_delta_ratio` | 0.60 | Requires close in the top/bottom 20% of bar range |
| `min_vol_ratio` | 1.30 | Requires volume 30% above average |
| `required_stacked` | 2 | Two consecutive strong bars minimum |

---

## 6. Family 5 — ML Prediction

### Entry rule

After walk-forward validation passes, the predictor emits `P(up)` on every
bar. A `Prediction` signal fires when:
- `P(up) ≥ 0.60` → Long
- `P(up) ≤ 0.40` → Short
- Otherwise → no signal

The thresholds are `probability_threshold` and `1 − probability_threshold`.
0.60 is the default because 0.55 produces too many signals and 0.65 too
few. Both the threshold and the model must clear walk-forward validation
— a model with AUC 0.52 does not pass even at threshold 0.60, because
the implied edge at threshold 0.60 with base AUC 0.52 is negligible.

### What "P(up)" actually means

`P(close[t + horizon] − close[t] > 0.30 × ATR[t])`. That is, the
probability that price rises by more than 30% of one ATR over the next
30 bars. It is **not** "probability price goes up at all" — the label has
a threshold, which is why the model can have apparent 55% directional
accuracy but only 30% actionable signal rate.

### Why no point prediction

See `docs/VALIDATION.md` §6. Short answer: at 1-minute resolution, gold's
noise is larger than any predictable signal by 10–100×. A point
prediction would be off by 20+ times the spread; the threshold-based
binary classification is the only prediction with measurable edge.

### Known failure modes

- **Regime shift:** a model trained on 2023 fails on 2024 if the market
  character changes. This is why walk-forward has per-regime breakdowns.
- **Calibration drift:** Platt scaling fitted on 15% of the data may not
  hold on new data if the base rate changes.
- **Threshold overfitting:** if you tune `probability_threshold` against
  the backtest, you are overfitting — walk-forward must be re-run.

---

## 7. Position management

Every entry uses the same management rules regardless of which family
fired the signal.

### Stop loss

`SL = entry − 1.5 × ATR` (longs) or `entry + 1.5 × ATR` (shorts)

**Why 1.5 × ATR?**

- Too tight (< 1×ATR): normal noise stops you out — win rate collapses.
- Too wide (> 3×ATR): losses hurt more than wins help — expectancy goes
  negative even at 60% win rate.
- 1.5×ATR is the empirical sweet spot on gold 1-minute: 1.5 removes most
  noise stops while keeping losing trades small enough to survive a
  losing streak.

### Take profit

`TP = entry + 3.0 × ATR` (longs) or `entry − 3.0 × ATR` (shorts)

**Why 3.0 × ATR (1:2 RR)?**

At 1:2 risk-reward, you break even at ~33% win rate (before costs). At
the observed ~55% win rate of the combined families, 1:2 RR yields
positive expectancy. A 1:1.5 RR raises the win rate needed to ~40% but
reduces per-trade reward, and on gold the win rate distribution at 1:1.5
is only slightly higher than at 1:2. Net: 1:2 is the better ratio here.

### Trailing stop

Once +1×ATR favorable:
- SL moves to breakeven (entry)

Then continuously:
- SL trails at `entry_extreme − 1.0 × ATR` (longs)
- SL only moves in the favorable direction (ratchet)

**Why breakeven at +1×ATR?** Because the trade has already proven its
thesis, and a loser that becomes a scratch is much better than a scratch
that becomes a loser. Without BE, the SL stays at 1.5×ATR forever and
~40% of would-be-small-wins become small-losses.

**Why trail at 1×ATR?** Tighter (0.5×ATR) trails out too early and
converts winning trades into scratches. Wider (2×ATR) gives back too much
of the gain when the trend reverses.

### Partial exit

At +1.5 × ATR favorable: close 50% of the position.
Move SL to breakeven on the remainder.

**Why partial?** Because gold can give back a full 3×ATR TP in a few
minutes. Locking in half at 1.5×ATR converts a "maybe" into "some".
The backtest improvement from partials is +5–10% on total PnL.

### Time stop

Force-close if `bar.ts − entry_ts > max_seconds_in_trade` (default 1800 = 30 min).

**Why wall-clock, not bars?** Because bars are irregular during
weekends/holidays. A 30-minute wall-clock limit means 30 real minutes,
not "30 bars that might span a weekend." The bar-count version would
hold positions through Friday night into Monday, which is never intended.

### Default parameters and why

| Param | Value | Rationale |
|---|---|---|
| `atr_period` | 14 | Wilder's default; matches RSI/ADX for consistency |
| `atr_sl_mult` | 1.5 | Empirical minimum to survive gold noise |
| `atr_tp_mult` | 3.0 | 1:2 RR — see above |
| `trail_after_atr` | 1.0 | BE trigger before trail — proven profit first |
| `trail_atr_mult` | 1.0 | Standard trail; tighter = more scratches |
| `partial_atr_mult` | 1.5 | Half-way to TP — captures "some" before "all" |
| `max_seconds_in_trade` | 1800 | 30 minutes — beyond that, thesis is stale |

---

## 8. Risk model

### Position sizing

```
units = min(
    (equity × risk_per_trade) / (atr_sl_mult × ATR),   ← risk-based
    (equity × max_leverage) / entry_price,             ← leverage cap
)
```

**Why ATR-scaled?**

Because a "1% risk" trade on a $4 ATR day is very different from a "1%
risk" trade on a $40 ATR day. ATR-scaling makes the risk proportional to
the volatility, so a single stop-out costs the same *percent* of equity
regardless of regime.

Fixed lot sizing ignores this and produces catastrophic days when
volatility spikes.

### Daily halt

`if equity < day_start_equity × (1 − daily_loss_limit) → halt for the day`

**Why 2%?**

Because 2% is enough to absorb a normal losing streak (3–4 stops) without
halting, but small enough that a truly bad day (news shock, strategy
failure) is capped early. A 5% halt is too generous; a 1% halt triggers
on normal variance.

Once halted, no new trades until midnight UTC. Existing positions are
managed normally (SL/TP still work).

### Weekly halt (not implemented)

A weekly halt at −5% would be the natural next step. Not currently in
the code — the daily halt is the only hard risk limit.

### Why `max_trades_per_day = 8`

Because beyond 8 trades, the edge per trade drops below costs. The 9th
and 10th trades of a day are usually the ones chasing losses. The 8-trade
cap enforces discipline.

### Why `max_seconds_in_trade` (wall clock) and not `max_bars_in_trade`

See §7. Bars are not minutes during closures. Wall-clock is the only
meaningful measure.

---

## 9. Parameter tuning guidance

### Which parameters matter most

Ranked by impact on backtest results:

1. `atr_sl_mult` — the single biggest lever. Small changes shift win rate
   by 10%+.
2. `atr_tp_mult` — sets the RR, which sets the required win rate.
3. `adx_min` — filters 40–60% of signals; a small change removes or adds
   hundreds of trades.
4. `probability_threshold` — if the predictor is active, this dominates
   everything else.
5. `rsi_low` / `rsi_high` — meaningful but constrained (25/75 or 30/70
   are the only sane choices).

### Which are fragile (overfit quickly)

- `squeeze_lookback` — anything from 2 to 5 "looks good" in backtest;
  the difference is noise.
- `min_delta_ratio` — the order-flow proxy is weak enough that tuning it
  is mostly fitting noise.
- `trail_atr_mult` — smooth function; 0.8 to 1.2 is all "fine" — the
  specific value barely matters.

### Which to leave alone

- `rsi_period`, `adx_period`, `atr_period`, `bb_period`, `kc_period` —
  these are calibrated to standard TA conventions. Changing them without
  a specific reason is usually a mistake.
- `trail_after_atr` — 1.0 works. 0.5 or 2.0 do not.
- `partial_atr_mult` — 1.5 is the only value that produces meaningful
  partials. 1.0 is too early, 2.0 is too late.

### What not to tune

**Never tune `probability_threshold` against the backtest.** If you
change it and rerun walk-forward validation, the validation loses
meaning — you have fit the threshold to the holdout. Pick a threshold
*once* (0.60 is standard), commit to it, and let walk-forward tell you
whether the model at that threshold is worth trading.

---

## 10. What this strategy is NOT

- **Not a scalping bot for the news.** The news blackout exists precisely
  because news bars are unmanageable.
- **Not a martingale.** No lot-size multiplier, no averaging into losers.
- **Not a grid.** Every trade has a fixed SL; no grid sits behind it.
- **Not a model that predicts price.** The predictor outputs a probability
  of a binary event, not a price.
- **Not a portfolio of uncorrelated bets.** All five families trade the
  same asset. Correlation is total; only regime coverage differs.
- **Not a hedge against anything.** Long-only or short-only per signal;
  no offsetting positions.

---

## 11. Reading the backtest output

When you see:

```
───── BACKTEST RESULTS ─────
Trades:       312
Win rate:     54.2%
```

Ask in this order:

1. **Is the win rate above break-even?** For 1:2 RR, break-even is ~35%
   (before costs) or ~40% (with costs). 54% clears it.
2. **Is the sample size enough?** 312 trades is marginal. >500 is better.
3. **Is the CI meaningful?** A 95% CI of [48.6%, 59.6%] crossing
   break-even means the win rate is indistinguishable from 47.4% — the
   strategy has no proven edge on this data.
4. **Did walk-forward pass?** If the predictor is disabled (check the
   warnings), the results only cover the four rule-based families.

A backtest that "made money" but whose statistical significance section
is full of warnings is a backtest that cannot be trusted. The
significance section is the actual result; the PnL line is one number
under the assumption of those significance tests passing.