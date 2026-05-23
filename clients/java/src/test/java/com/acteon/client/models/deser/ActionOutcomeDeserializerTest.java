package com.acteon.client.models.deser;

import com.acteon.client.JsonMapper;
import com.acteon.client.models.ActionOutcome;
import com.acteon.client.models.ActionOutcome.OutcomeType;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

class ActionOutcomeDeserializerTest {
    private static final ObjectMapper MAPPER = JsonMapper.build();

    @Test
    void executedVariantCarriesProviderResponse() throws Exception {
        String json = """
            {
              "Executed": {
                "status": "success",
                "body": {"sent": true},
                "headers": {"X-Trace": "abc"}
              }
            }
            """;

        ActionOutcome outcome = MAPPER.readValue(json, ActionOutcome.class);

        assertEquals(OutcomeType.EXECUTED, outcome.getType());
        assertNotNull(outcome.getResponse());
        assertEquals("success", outcome.getResponse().getStatus());
        assertEquals(true, outcome.getResponse().getBody().get("sent"));
    }

    @Test
    void deduplicatedAsBareString() throws Exception {
        ActionOutcome outcome = MAPPER.readValue("\"Deduplicated\"", ActionOutcome.class);

        assertEquals(OutcomeType.DEDUPLICATED, outcome.getType());
    }

    @Test
    void emptyObjectIsDeduplicated() throws Exception {
        // The pre-migration fromMap treated {} as Deduplicated.
        ActionOutcome outcome = MAPPER.readValue("{}", ActionOutcome.class);

        assertEquals(OutcomeType.DEDUPLICATED, outcome.getType());
    }

    @Test
    void suppressedCarriesRule() throws Exception {
        String json = """
            {"Suppressed": {"rule": "block-spam"}}
            """;

        ActionOutcome outcome = MAPPER.readValue(json, ActionOutcome.class);

        assertEquals(OutcomeType.SUPPRESSED, outcome.getType());
        assertEquals("block-spam", outcome.getRule());
    }

    @Test
    void reroutedCarriesProvidersAndOptionalResponse() throws Exception {
        String json = """
            {
              "Rerouted": {
                "original_provider": "email",
                "new_provider": "sms",
                "response": {"status": "success", "body": {}}
              }
            }
            """;

        ActionOutcome outcome = MAPPER.readValue(json, ActionOutcome.class);

        assertEquals(OutcomeType.REROUTED, outcome.getType());
        assertEquals("email", outcome.getOriginalProvider());
        assertEquals("sms", outcome.getNewProvider());
        assertNotNull(outcome.getResponse());
    }

    @Test
    void throttledCarriesRustDuration() throws Exception {
        String json = """
            {"Throttled": {"retry_after": {"secs": 5, "nanos": 500000000}}}
            """;

        ActionOutcome outcome = MAPPER.readValue(json, ActionOutcome.class);

        assertEquals(OutcomeType.THROTTLED, outcome.getType());
        assertEquals(5_500_000_000L, outcome.getRetryAfter().toNanos());
    }

    @Test
    void failedCarriesActionError() throws Exception {
        String json = """
            {"Failed": {"code": "TIMEOUT", "message": "timed out", "retryable": true, "attempts": 3}}
            """;

        ActionOutcome outcome = MAPPER.readValue(json, ActionOutcome.class);

        assertEquals(OutcomeType.FAILED, outcome.getType());
        assertEquals("TIMEOUT", outcome.getError().getCode());
        assertTrue(outcome.getError().isRetryable());
        assertEquals(3, outcome.getError().getAttempts());
    }

    @Test
    void dryRunCarriesVerdict() throws Exception {
        String json = """
            {"DryRun": {"verdict": "allow", "matched_rule": "default", "would_be_provider": "email"}}
            """;

        ActionOutcome outcome = MAPPER.readValue(json, ActionOutcome.class);

        assertEquals(OutcomeType.DRY_RUN, outcome.getType());
        assertEquals("allow", outcome.getVerdict());
        assertEquals("default", outcome.getMatchedRule());
        assertEquals("email", outcome.getWouldBeProvider());
    }

    @Test
    void scheduledCarriesActionId() throws Exception {
        String json = """
            {"Scheduled": {"action_id": "act-123", "scheduled_for": "2026-04-17T12:00:00Z"}}
            """;

        ActionOutcome outcome = MAPPER.readValue(json, ActionOutcome.class);

        assertEquals(OutcomeType.SCHEDULED, outcome.getType());
        assertEquals("act-123", outcome.getActionId());
        assertEquals("2026-04-17T12:00:00Z", outcome.getScheduledFor());
    }

    @Test
    void quotaExceededCarriesLimits() throws Exception {
        String json = """
            {"QuotaExceeded": {"tenant": "acme", "limit": 1000, "used": 1001, "overage_behavior": "reject"}}
            """;

        ActionOutcome outcome = MAPPER.readValue(json, ActionOutcome.class);

        assertEquals(OutcomeType.QUOTA_EXCEEDED, outcome.getType());
        assertEquals("acme", outcome.getTenant());
        assertEquals(1000, outcome.getQuotaLimit());
        assertEquals(1001, outcome.getQuotaUsed());
        assertEquals("reject", outcome.getOverageBehavior());
    }

    @Test
    void unknownVariantBecomesFailedWithSyntheticError() throws Exception {
        // The pre-migration fromMap mapped unknown variants to a
        // Failed outcome rather than throwing — preserve that
        // contract so callers' existing defensive switches keep
        // compiling.
        String json = """
            {"FutureVariantTheClientDoesntKnowAbout": {}}
            """;

        ActionOutcome outcome = MAPPER.readValue(json, ActionOutcome.class);

        assertEquals(OutcomeType.FAILED, outcome.getType());
        assertEquals("UNKNOWN", outcome.getError().getCode());
    }
}
