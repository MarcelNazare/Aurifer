# ─────────────────────────────────────────────────────────────────────
#  Aurifer — Makefile
#  A scientific instrument for gold (XAU/USD) trading strategies.
#  It tells you when not to trade.
# ─────────────────────────────────────────────────────────────────────

BINARY  := aurifer
TARGET  := target/release/$(BINARY)
START   ?= 2024-01-01
END     ?= 2024-03-01

# ANSI colors for a friendlier output
CYAN    := \033[36m
GREEN   := \033[32m
YELLOW  := \033[33m
RED     := \033[31m
RESET   := \033[0m

.DEFAULT_GOAL := help
.PHONY: help build run web backtest diagnostics live check test fmt clippy docs clean rebuild

# ── Help ─────────────────────────────────────────────────────────────

help:
	@echo "$(CYAN)Aurifer$(RESET) — a scientific instrument for gold trading"
	@echo ""
	@echo "Usage: $(GREEN)make <target>$(RESET)"
	@echo ""
	@echo "  $(CYAN)build$(RESET)        Compile release binary → $(TARGET)"
	@echo "  $(CYAN)run$(RESET)          Interactive TUI (default command)"
	@echo "  $(CYAN)web$(RESET)          Browser dashboard at http://localhost:<port>"
	@echo "  $(CYAN)backtest$(RESET)     Headless backtest with statistical report"
	@echo "  $(CYAN)diagnostics$(RESET)  Point-prediction diagnostic (proves it's untradeable)"
	@echo "  $(CYAN)live$(RESET)         OANDA demo live trading (requires --confirm-live)"
	@echo ""
	@echo "  $(CYAN)check$(RESET)        Fast type-check without full build"
	@echo "  $(CYAN)test$(RESET)         Run test suite"
	@echo "  $(CYAN)fmt$(RESET)          Format source with rustfmt"
	@echo "  $(CYAN)clippy$(RESET)       Lint with clippy (deny warnings)"
	@echo "  $(CYAN)docs$(RESET)         Generate API docs, open in browser"
	@echo "  $(CYAN)clean$(RESET)        Remove build artifacts"
	@echo "  $(CYAN)rebuild$(RESET)      clean + build"
	@echo ""
	@echo "Date range (START / END), e.g.:"
	@echo "  make backtest START=2023-01-01 END=2023-12-31"

# ── Build ────────────────────────────────────────────────────────────

build:
	@echo "$(CYAN)→$(RESET) building release binary"
	cargo build --release
	@echo "$(GREEN)✓$(RESET) $(TARGET) ($(shell du -h $(TARGET) 2>/dev/null | cut -f1 || echo '—'))"

rebuild: clean build

# ── Run ──────────────────────────────────────────────────────────────

run: build
	@echo "$(CYAN)→$(RESET) launching TUI"
	./$(TARGET) --start $(START) --end $(END)

web: build
	@echo "$(CYAN)→$(RESET) launching web dashboard"
	./$(TARGET) web --start $(START) --end $(END)

backtest: build
	@echo "$(CYAN)→$(RESET) running backtest ($(START) → $(END))"
	./$(TARGET) backtest --start $(START) --end $(END)

diagnostics: build
	@echo "$(CYAN)→$(RESET) point-prediction diagnostic ($(START) → $(END))"
	./$(TARGET) diagnostics point-prediction --start $(START) --end $(END)

live: build
	@echo "$(YELLOW)⚠  OANDA demo mode. Real money requires OANDA_LIVE=1$(RESET)"
	@echo "$(YELLOW)⚠  Read docs/LIVE_TRADING.md first.$(RESET)"
	./$(TARGET) live --confirm-live --start $(START) --end $(END)

# ── Development ──────────────────────────────────────────────────────

check:
	cargo check --all-targets

test:
	cargo test --all

fmt:
	cargo fmt --all

clippy:
	cargo clippy --all-targets -- -D warnings

docs:
	cargo doc --no-deps --open

clean:
	cargo clean
	@echo "$(GREEN)✓$(RESET) cleaned"