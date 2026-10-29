# Validation

Why the program says "DO NOT TRADE LIVE" — and why that's the whole point.

---

## 1. Why validation matters more than backtest results

A backtest that "made 20%" is a number. A backtest that "made 20% and
survived walk-forward validation on a holdout set" is *evidence*. These
are fundamentally different claims, and the difference is what separates
research from gambling.

The number of ways a backtest can lie is enormous:

- **Lookahead bias** — features that somehow see the future
- **Overfitting** — parameters tuned to the specific historical window
- **Survivorship** — data that happens to capture a favorable period
- **Cost understatement** — assuming tighter spread than reality
- **Fill assumption** — filling at a price you couldn't actually get
- **Sample size** — 30 trades can look amazing by luck
- **Regime dependence** — works in one market regime, dies in the next

Validation is the set of statistical and methodological checks that try
to catch all seven. Aurifer runs them automatically. When it says "DO
NOT TRADE LIVE," it is telling you that one or more of these checks
failed.

---

## 2. The three-way split

For any model that produces probabilities, the data must be split
**three ways**:

| Split | Fraction | Purpose |
|---|---|---|
| Training | 60% | Fit the model parameters |
| Calibration | 15% | Fit Platt scaling — make "60%" mean 60% |
| Test | 25% | Report metrics on data the model never saw |

**Why not just train/test?** Because a model fit on 60% of the data has
probabilities that are systematically biased. If you train a logistic
regression on 60% and evaluate it on the remaining 40%, "P(up) = 0.60"
might correspond to a true 45% frequency. Platt scaling on a held-out
15% corrects this bias; then the final 25% measures the calibrated
model.

**Why 25% holdout is the minimum?** For a bootstrap CI on AUC to be
meaningful, the test set needs hundreds of samples. On 1-minute gold,
6 months of data (≈130,000 bars, minus label-loss at the horizon) yields
~30,000 usable samples; 25% of that is plenty. On 1 month of data,
25% may be under 500 samples and the CI becomes very wide. **If your
holdout has fewer than 500 samples, the test metrics cannot be trusted.**

---

## 3. Walk-forward validation

### Why expanding window

A rolling-window backtest (e.g., "train on 30 days, test on next 5")
throws away data. Expanding window keeps all history:

```
Fold 1:  [0 ... 20%]  train       [20% ... 40%]  test
Fold 2:  [0 ... 40%]  train       [40% ... 60%]  test
Fold 3:  [0 ... 60%]  train       [60% ... 80%]  test
Fold 4:  [0 ... 80%]  train       [80% ... 100%] test
```

Each fold trains on all data available up to its train-end, then tests on
a contiguous window that comes strictly after. No test window overlaps
with any training data.

**Why 4 folds?**

- 2 folds: too little evidence; one bad fold poisons the average.
- 3 folds: the minimum for a "worst fold < 0.50" rejection to be meaningful.
- 4 folds: 4 × 20% = 80% train + 4 × 20% test (with overlap) — each fold
  tests on a large enough chunk to have samples.
- 5+ folds: diminishing returns; each test window becomes too small.

4 is the sweet spot for 1-minute gold with 6 months of data.

### Per-regime breakdown

Each test bar is classified into one of four regimes:

| Regime | Condition |
|---|---|
| `TREND_UP` | ADX ≥ 25 and price above trend EMA |
| `TREND_DN` | ADX ≥ 25 and price below trend EMA |
| `RANGING` | ADX < 20 |
| `TRANSITION` | 20 ≤ ADX < 25 |

The model's AUC is computed separately within each regime.

**Why this matters:** a model with aggregate AUC 0.58 but zero AUC in
RANGING is not "a good model" — it is "a model that works in trends and
fails in chop." If your account trades through all regimes (it will), the
overall performance is a blend, and the model's average hides that it
loses money half the time.

### Auto-disable thresholds

`walk_forward_validate` returns `Ok` only if **all** three conditions hold:

1. **Aggregate AUC ≥ 0.54.** Anything below is indistinguishable from
   random given typical gold noise.
2. **Worst fold AUC ≥ 0.50.** If any fold is below random, the model has
   a regime where it is actively wrong — a red flag for real-money use.
3. **≥ 2 regimes with AUC > 0.52 and n ≥ 100.** A model that works in
   only one regime is unusable in practice; you don't get to choose which
   regime the market is in.

If any condition fails, `main.rs` proceeds with the predictor disabled.
The backtest still runs, but only the four rule-based families are
active. This is intentional: **a model that fails validation should not
be quietly attached at reduced weight — it should be turned off.**

---

## 4. Calibration

### Why raw probabilities lie

A logistic regression fit on 60% of the data has weights tuned to minimize
loss on that 60%. On new data, the output probabilities tend to be
overconfident — the model says "70%" when the true frequency is 55%.

If you trade a threshold rule (`p ≥ 0.60 → long`), the threshold is
meaningless unless `p = 0.60` actually corresponds to a 60% frequency of
the label being true.

### Platt scaling

Platt scaling fits a 1-D logistic regression on the raw model's output:

```
p_calibrated = sigmoid(a × logit_raw + b)
```

Where `logit_raw = log(p_raw / (1 − p_raw))`. The two parameters `(a, b)`
are fit on the calibration slice (15% of data) by minimizing cross-entropy.

**In practice:**

- `a ≈ 1, b ≈ 0` → the raw model was already calibrated
- `a < 1` → the raw model was overconfident (typical)
- `a < 0` → **the raw model was anti-correlated with truth**, which
  usually means it was overfit or a bug exists in feature extraction

Aurifer prints `Platt a,b` after training. If `a < 0`, treat it as a bug.

### Brier score

`Brier = mean((p_predicted − y_actual)²)` where `y_actual ∈ {0, 1}`.

- **Brier 0.25** = guessing always 0.5
- **Brier 0.20** = decent probabilistic forecast
- **Brier 0.15** = very good
- **Brier 0.10** = excellent (rare in finance)

The threshold in Aurifer is **0.24**. Above that, the probabilities are
not informative enough to trade on. This is a loose threshold — a
Brier of 0.22 that clears the threshold is not "good," just "not
disqualified."

### Why calibration matters for thresholds

The trading rule uses `p ≥ 0.60 → long`. If the model is calibrated, this
trades exactly the subset of bars where the true probability is ≥ 60%.
If it is not calibrated, it might trade bars where the true probability
is 45% — and lose money at a rate the backtest never predicts.

---

## 5. Statistical significance

Every backtest ends with a significance report. This is the section that
decides whether the strategy is tradeable at all.

### Wilson CI on win rate

The naive "win rate ± 1.96 × sqrt(p × (1−p) / n)" is a normal
approximation that behaves badly for small `n` and for `p` near 0 or 1.
Wilson's score interval is bounded to [0, 1], performs well for all `n`,
and is the standard choice for binomial proportions.

The 95% Wilson CI is reported as `[lower, upper]`. If `lower > breakeven_winrate`,
the strategy has a **statistically significant edge** at the 95% level.

### Break-even win rate

Computed from the actual average win/loss in the trades:

```
breakeven_winrate = avg_loss / (avg_win + avg_loss)
```

For a 1:2 RR strategy, avg_win ≈ 2 × avg_loss, so breakeven ≈ 1/3 ≈ 33%.
With costs, avg_win shrinks and avg_loss grows, so breakeven moves
toward ~40%.

The comparison is `win_rate_ci.0 > breakeven_winrate`. If `false`, the
win rate is not significantly better than what would be needed to break
even.

### Bootstrap CI on PnL and Sharpe

The trades' PnL values are resampled with replacement 1000 times. Each
resample produces a total PnL and a Sharpe ratio. The 2.5th and 97.5th
percentiles of those resamples form the 95% CI.

- **PnL CI that crosses zero** → the strategy's total PnL is
  indistinguishable from zero on this data.
- **Sharpe CI that includes negatives** → the risk-adjusted edge is not
  proven.

### The warning conditions

The report flags a warning when:

1. `n_trades < 100` — sample too small
2. `win_rate_ci.0 ≤ breakeven_winrate` (with n ≥ 100) — no significant
   edge
3. `pnl_ci.0 < 0 < pnl_ci.1` — PnL CI crosses zero
4. `sharpe_ci.0 < 0` — Sharpe CI includes negatives

**If any warning fires, the report ends with:**

```
🚫 DO NOT TRADE LIVE until significance warnings clear.
```

This is not a suggestion. It means: on this data, the strategy's result
is indistinguishable from luck.

---

## 6. The point-prediction diagnostic

`aurifer diagnostics point-prediction` exists to demonstrate — with your
own data — why point prediction is untradeable.

### What it does

Trains a ridge regression to predict `close[t + horizon] − close[t]` in
dollars. Then evaluates on a held-out 30%:

- Mean absolute error (MAE)
- Median absolute error
- RMSE
- % of predictions within half-spread
- % within spread
- % within typical SL
- Actionable PnL: what if you traded every prediction above cost?

### What it proves

For the strategy to profit from point prediction, >90% of predictions must
be accurate within half-spread. Because:

- If the prediction is off by more than the round-trip cost, you lose
  even when the direction is right.
- Half-spread is the one-way cost; if prediction error > half-spread,
  the fill will be worse than the predicted price, eating the edge.

On 1-minute gold, typical MAE is **$2–$5**, while spread is **$0.20**.
That is a 10–25× gap. The diagnostic prints the verdict as
`❌ NOT TRADEABLE` because the accuracy needed (90%+) is off by an order
of magnitude from what any model achieves (3–8%).

### Why this is important

Because the "predict gold price" idea is intuitive and appealing, and it
fails for a mathematical reason that can be shown in 20 lines of output.
The diagnostic exists to show that reason — so that no one wastes weeks
trying to make point prediction work.

---

## 7. What validation does NOT prove

Validation is necessary but not sufficient. It catches the seven ways a
backtest lies (see §1), but it cannot catch:

### In-sample significance ≠ out-of-sample edge

A validation pass on your 6-month sample proves the model *on that
sample*. It says nothing about the next 6 months. To get closer, you
need:

- A different calendar year
- A different regime (e.g., a trending period if your sample was ranging)
- A different asset (does the edge generalize?)

If the strategy passes on one year and fails on the next, it does not
have a tradeable edge — it had a lucky year.

### Out-of-sample edge ≠ live profitability

Live trading introduces frictions the backtest cannot model:

- **Slippage is worse** — the modeled `0.05` is often `0.10+` live
- **Fill delays** — even at next-bar-open, execution takes 50–200ms
- **Partial fills** — a 10-unit order might fill as 3 + 4 + 3 at different prices
- **Rejections** — the broker can reject orders during volatility
- **Disconnects** — the network drops mid-trade

Every one of these degrades performance relative to the backtest. A
strategy that is marginal in backtest becomes losing live.

### A passing validation means "not obviously luck"

It does not mean "profitable." It means "the statistical tests did not
flag this as indistinguishable from noise." That is a much lower bar.

### What to do if you want more confidence

1. **Run on 3+ years of data.** One year is not enough.
2. **Run on a different calendar year.** If both pass, the edge may be real.
3. **Demo-trade for 3 months.** Compare demo results against the backtest
   on the same period. If demo loses money where the backtest was
   profitable, the backtest is wrong.
4. **Check the realized adverse gap.** Printed at the end of every
   backtest. If it's > 1.5× the modeled slippage, the model was too
   optimistic — raise `Config::slippage` and re-run.

If any of those fail, the strategy does not have a tradeable edge. The
correct response is more research — not more leverage, not tighter stops,
not a bigger sample.

---

## 8. The multiple-year test

### Why one year is not enough

Gold's behavior varies dramatically across regimes:

| Period | Regime | Character |
|---|---|---|
| 2020 | COVID shock | Extreme volatility, directional |
| 2021 | Post-COVID | Range-bound, choppy |
| 2022 | Inflation / rate hikes | Strong downtrend |
| 2023 | Consolidation | Range-bound, low vol |
| 2024 | Fed-cut expectations | Strong uptrend |

A strategy that works in 2023 (ranging) and 2024 (uptrend) is more
likely to generalize than one that only works in 2020 (extreme). Testing
across different regimes is not optional if you want confidence.

### How to test across regimes

1. Download 3+ years of 1-minute data (requires Databento or Polygon —
   IG caps at ~1000 bars per request)
2. Run `aurifer backtest --start 2022-01-01 --end 2024-12-31`
3. Read the walk-forward per-regime breakdown. If all four regimes show
   positive edge, the model is robust. If only one does, it's regime-specific.
4. Rerun on each individual year. If any single year is catastrophic
   (large negative PnL, AUC below 0.50), the strategy has a failure mode
   you have not addressed.

### What to do when year 2 fails

**Do not tune parameters to fix it.** That is overfitting to the failing
year. Instead:

1. **Diagnose:** is the failure in a specific regime? If so, the strategy
   needs a regime filter, not parameter changes.
2. **Question the hypothesis:** does the edge exist at all, or was year 1
   luck?
3. **Accept:** most strategies do not generalize. A failure on year 2 is
   information, not a problem to solve.

The most common mistake in retail algo trading is treating a backtest
failure as "tune it until it passes." The correct interpretation is "this
did not work as-is; is the underlying edge real or not?" — and if the
answer is "not clear," the strategy is not tradeable.