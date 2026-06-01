# TLA+ Model Checking Makefile
#
# Usage:
#   make check-all          # Run all specs (CI default)
#   make check-circuit      # Run CircuitBreaker only
#   make check-dedup        # Run DispatchDedup only
#   make check-hashchain    # Run HashChain only
#   make check-recurring    # Run RecurringDispatch only
#   make check-bus          # Run MessageBus only
#   make check-chain        # Run ChainOrdering only
#   make check-approval     # Run ApprovalLifecycle only
#   make check-quota        # Run QuotaCounter only
#   make check-busapproval  # Run BusApproval only
#   make check-multiquota   # Run MultiQuotaRollback only
#   make check-task         # Run A2aTaskTransition only
#   make check-cancel       # Run ChainCancelCascade only
#   make check-replay       # Run StreamReplay only
#   make check-reaper       # Run RetentionReaper only
#   make check-dlq          # Run DlqRedelivery only
#   make check-msgdedup     # Run MessageDedup only
#   make check-keyrotation  # Run KeyRotation only
#   make check-refgraph     # Run RefGraphDefense only
#   make setup              # Download tla2tools.jar
#   make clean              # Remove downloaded tools

SHELL := /bin/bash
SPECS_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
CI_SCRIPT := $(SPECS_DIR)ci/run-tlc.sh

.PHONY: check-all check-circuit check-dedup check-hashchain check-recurring check-bus check-chain check-approval check-quota check-busapproval check-multiquota check-task check-cancel check-replay check-reaper check-dlq check-msgdedup check-keyrotation check-refgraph setup clean help

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

check-all: ## Run TLC on all specs
	@$(CI_SCRIPT)

check-circuit: ## Run TLC on CircuitBreaker spec
	@$(CI_SCRIPT) CircuitBreaker

check-dedup: ## Run TLC on DispatchDedup spec
	@$(CI_SCRIPT) DispatchDedup

check-hashchain: ## Run TLC on HashChain spec
	@$(CI_SCRIPT) HashChain

check-recurring: ## Run TLC on RecurringDispatch spec
	@$(CI_SCRIPT) RecurringDispatch

check-bus: ## Run TLC on MessageBus spec
	@$(CI_SCRIPT) MessageBus

check-chain: ## Run TLC on ChainOrdering spec
	@$(CI_SCRIPT) ChainOrdering

check-approval: ## Run TLC on ApprovalLifecycle spec
	@$(CI_SCRIPT) ApprovalLifecycle

check-quota: ## Run TLC on QuotaCounter spec
	@$(CI_SCRIPT) QuotaCounter

check-busapproval: ## Run TLC on BusApproval spec
	@$(CI_SCRIPT) BusApproval

check-multiquota: ## Run TLC on MultiQuotaRollback spec
	@$(CI_SCRIPT) MultiQuotaRollback

check-task: ## Run TLC on A2aTaskTransition spec
	@$(CI_SCRIPT) A2aTaskTransition

check-cancel: ## Run TLC on ChainCancelCascade spec
	@$(CI_SCRIPT) ChainCancelCascade

check-replay: ## Run TLC on StreamReplay spec
	@$(CI_SCRIPT) StreamReplay

check-reaper: ## Run TLC on RetentionReaper spec
	@$(CI_SCRIPT) RetentionReaper

check-dlq: ## Run TLC on DlqRedelivery spec
	@$(CI_SCRIPT) DlqRedelivery

check-msgdedup: ## Run TLC on MessageDedup spec
	@$(CI_SCRIPT) MessageDedup

check-keyrotation: ## Run TLC on KeyRotation spec
	@$(CI_SCRIPT) KeyRotation

check-refgraph: ## Run TLC on RefGraphDefense spec
	@$(CI_SCRIPT) RefGraphDefense

setup: ## Download tla2tools.jar (requires curl or wget)
	@$(CI_SCRIPT) __nonexistent__ 2>/dev/null || true
	@echo "Setup complete. Tools in $(SPECS_DIR).tools/"

clean: ## Remove downloaded tools
	rm -rf $(SPECS_DIR).tools/
