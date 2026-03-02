# TLA+ Model Checking Makefile
#
# Usage:
#   make check-all          # Run all specs (CI default)
#   make check-circuit      # Run CircuitBreaker only
#   make check-dedup        # Run DispatchDedup only
#   make setup              # Download tla2tools.jar
#   make clean              # Remove downloaded tools

SHELL := /bin/bash
SPECS_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
CI_SCRIPT := $(SPECS_DIR)ci/run-tlc.sh

.PHONY: check-all check-circuit check-dedup setup clean help

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

check-all: ## Run TLC on all specs
	@$(CI_SCRIPT)

check-circuit: ## Run TLC on CircuitBreaker spec
	@$(CI_SCRIPT) CircuitBreaker

check-dedup: ## Run TLC on DispatchDedup spec
	@$(CI_SCRIPT) DispatchDedup

setup: ## Download tla2tools.jar (requires curl or wget)
	@$(CI_SCRIPT) __nonexistent__ 2>/dev/null || true
	@echo "Setup complete. Tools in $(SPECS_DIR).tools/"

clean: ## Remove downloaded tools
	rm -rf $(SPECS_DIR).tools/
