//go:build ignore

// Test script for the Go Acteon client.
//
// Usage:
//
//	ACTEON_URL=http://localhost:8080 go run test_go_client.go
package main

import (
	"context"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/penserai/acteon/clients/go/acteon"
)

type testResult struct {
	passed int
	failed int
}

func main() {
	baseURL := os.Getenv("ACTEON_URL")
	if baseURL == "" {
		baseURL = "http://localhost:8080"
	}

	fmt.Printf("Go Client Test - connecting to %s\n", baseURL)
	fmt.Println(strings.Repeat("=", 60))

	client := acteon.NewClient(baseURL, acteon.WithTimeout(30*time.Second))
	ctx := context.Background()
	results := &testResult{}

	test := func(name string, fn func() error) {
		if err := fn(); err != nil {
			fmt.Printf("  [FAIL] %s: %v\n", name, err)
			results.failed++
		} else {
			fmt.Printf("  [PASS] %s\n", name)
			results.passed++
		}
	}

	// Test: Health check
	test("Health()", func() error {
		healthy, err := client.Health(ctx)
		if err != nil {
			return err
		}
		if !healthy {
			return fmt.Errorf("health check returned false")
		}
		return nil
	})

	// Test: Single dispatch
	var dispatchedID string
	test("Dispatch()", func() error {
		action := acteon.NewAction(
			"test",
			"go-client",
			"email",
			"send_notification",
			map[string]any{"to": "test@example.com", "subject": "Go test"},
		)
		dispatchedID = action.ID
		outcome, err := client.Dispatch(ctx, action)
		if err != nil {
			return err
		}
		validTypes := map[acteon.OutcomeType]bool{
			acteon.OutcomeExecuted:     true,
			acteon.OutcomeDeduplicated: true,
			acteon.OutcomeSuppressed:   true,
			acteon.OutcomeRerouted:     true,
			acteon.OutcomeThrottled:    true,
			acteon.OutcomeFailed:       true,
		}
		if !validTypes[outcome.Type] {
			return fmt.Errorf("unexpected outcome type: %s", outcome.Type)
		}
		return nil
	})

	// Test: Batch dispatch
	test("DispatchBatch()", func() error {
		var actions []*acteon.Action
		for i := 0; i < 3; i++ {
			actions = append(actions, acteon.NewAction(
				"test",
				"go-client",
				"email",
				"batch_test",
				map[string]any{"seq": i},
			))
		}
		resultsList, err := client.DispatchBatch(ctx, actions)
		if err != nil {
			return err
		}
		if len(resultsList) != 3 {
			return fmt.Errorf("expected 3 results, got %d", len(resultsList))
		}
		return nil
	})

	// Test: List rules
	test("ListRules()", func() error {
		rules, err := client.ListRules(ctx)
		if err != nil {
			return err
		}
		// rules can be empty, just check it's a valid slice
		_ = rules
		return nil
	})

	// Test: Deduplication
	test("Deduplication", func() error {
		dedupKey := fmt.Sprintf("go-dedup-%s", uuid.New().String())
		action1 := acteon.NewAction(
			"test",
			"go-client",
			"email",
			"dedup_test",
			map[string]any{"msg": "first"},
		).WithDedupKey(dedupKey)
		action2 := acteon.NewAction(
			"test",
			"go-client",
			"email",
			"dedup_test",
			map[string]any{"msg": "second"},
		).WithDedupKey(dedupKey)

		outcome1, err := client.Dispatch(ctx, action1)
		if err != nil {
			return fmt.Errorf("first dispatch: %w", err)
		}
		_, err = client.Dispatch(ctx, action2)
		if err != nil {
			return fmt.Errorf("second dispatch: %w", err)
		}

		// First should execute or fail
		if outcome1.Type != "executed" && outcome1.Type != "failed" {
			return fmt.Errorf("unexpected first outcome: %s", outcome1.Type)
		}
		return nil
	})

	// Test: Query audit
	test("QueryAudit()", func() error {
		query := &acteon.AuditQuery{
			Tenant: "go-client",
			Limit:  10,
		}
		page, err := client.QueryAudit(ctx, query)
		if err != nil {
			return err
		}
		// Just check it's a valid response
		_ = page.Total
		_ = page.Records
		return nil
	})

	// Summary
	fmt.Println(strings.Repeat("=", 60))
	total := results.passed + results.failed
	fmt.Printf("Results: %d/%d passed\n", results.passed, total)

	if results.failed > 0 {
		os.Exit(1)
	}
	_ = dispatchedID // suppress unused warning
}
