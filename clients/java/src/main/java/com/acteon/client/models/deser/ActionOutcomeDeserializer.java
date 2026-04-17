package com.acteon.client.models.deser;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.databind.DeserializationContext;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.deser.std.StdDeserializer;

import com.acteon.client.models.ActionError;
import com.acteon.client.models.ActionOutcome;
import com.acteon.client.models.ActionOutcome.OutcomeType;
import com.acteon.client.models.ProviderResponse;

import java.io.IOException;
import java.time.Duration;
import java.util.Iterator;

/**
 * Decodes an {@link ActionOutcome} from the Rust serde adjacent-tagged
 * enum shape produced by the gateway:
 *
 * <ul>
 *   <li>{@code "Deduplicated"} — bare string variant</li>
 *   <li>{@code {"Executed": {"status": ..., "body": ..., "headers": ...}}}</li>
 *   <li>{@code {"Suppressed": {"rule": ...}}}</li>
 *   <li>{@code {"Rerouted": {"original_provider": ..., "new_provider": ..., "response": {...}}}}</li>
 *   <li>{@code {"Throttled": {"retry_after": {"secs": ..., "nanos": ...}}}}</li>
 *   <li>{@code {"Failed": {"code": ..., "message": ..., ...}}}</li>
 *   <li>{@code {"DryRun": {"verdict": ..., "matched_rule": ..., "would_be_provider": ...}}}</li>
 *   <li>{@code {"Scheduled": {"action_id": ..., "scheduled_for": ...}}}</li>
 *   <li>{@code {"QuotaExceeded": {"tenant": ..., "limit": ..., "used": ..., "overage_behavior": ...}}}</li>
 * </ul>
 *
 * <p>An empty object is treated as {@code Deduplicated} to match the
 * pre-migration {@code fromMap} behavior.
 *
 * <p>An unknown variant is mapped to a {@code Failed} outcome with a
 * synthetic {@code UNKNOWN} error rather than throwing — same contract
 * the old hand-written dispatcher provided so callers can keep their
 * defensive {@code switch} branches.
 */
public class ActionOutcomeDeserializer extends StdDeserializer<ActionOutcome> {
    public ActionOutcomeDeserializer() {
        super(ActionOutcome.class);
    }

    @Override
    public ActionOutcome deserialize(JsonParser p, DeserializationContext ctxt) throws IOException {
        JsonNode node = p.getCodec().readTree(p);
        ObjectMapper mapper = (ObjectMapper) p.getCodec();
        ActionOutcome outcome = new ActionOutcome();

        if (node.isTextual()) {
            String s = node.asText();
            if ("Deduplicated".equals(s)) {
                outcome.setType(OutcomeType.DEDUPLICATED);
                return outcome;
            }
            outcome.setType(OutcomeType.FAILED);
            outcome.setError(new ActionError("UNKNOWN", "Unknown outcome string: " + s, false, 0));
            return outcome;
        }

        if (!node.isObject() || node.isEmpty()) {
            outcome.setType(OutcomeType.DEDUPLICATED);
            return outcome;
        }

        Iterator<String> fields = node.fieldNames();
        if (!fields.hasNext()) {
            outcome.setType(OutcomeType.DEDUPLICATED);
            return outcome;
        }
        String variant = fields.next();
        JsonNode payload = node.get(variant);

        switch (variant) {
            case "Executed":
                outcome.setType(OutcomeType.EXECUTED);
                outcome.setResponse(mapper.treeToValue(payload, ProviderResponse.class));
                return outcome;
            case "Deduplicated":
                outcome.setType(OutcomeType.DEDUPLICATED);
                return outcome;
            case "Suppressed":
                outcome.setType(OutcomeType.SUPPRESSED);
                if (payload != null && payload.has("rule")) {
                    outcome.setRule(payload.get("rule").asText());
                }
                return outcome;
            case "Rerouted":
                outcome.setType(OutcomeType.REROUTED);
                if (payload != null) {
                    if (payload.has("original_provider")) {
                        outcome.setOriginalProvider(payload.get("original_provider").asText());
                    }
                    if (payload.has("new_provider")) {
                        outcome.setNewProvider(payload.get("new_provider").asText());
                    }
                    if (payload.has("response") && !payload.get("response").isNull()) {
                        outcome.setResponse(mapper.treeToValue(payload.get("response"), ProviderResponse.class));
                    }
                }
                return outcome;
            case "Throttled":
                outcome.setType(OutcomeType.THROTTLED);
                if (payload != null && payload.has("retry_after")) {
                    outcome.setRetryAfter(mapper.treeToValue(payload.get("retry_after"), Duration.class));
                }
                return outcome;
            case "Failed":
                outcome.setType(OutcomeType.FAILED);
                outcome.setError(mapper.treeToValue(payload, ActionError.class));
                return outcome;
            case "DryRun":
                outcome.setType(OutcomeType.DRY_RUN);
                if (payload != null) {
                    if (payload.has("verdict")) {
                        outcome.setVerdict(payload.get("verdict").asText());
                    }
                    if (payload.has("matched_rule") && !payload.get("matched_rule").isNull()) {
                        outcome.setMatchedRule(payload.get("matched_rule").asText());
                    }
                    if (payload.has("would_be_provider") && !payload.get("would_be_provider").isNull()) {
                        outcome.setWouldBeProvider(payload.get("would_be_provider").asText());
                    }
                }
                return outcome;
            case "Scheduled":
                outcome.setType(OutcomeType.SCHEDULED);
                if (payload != null) {
                    if (payload.has("action_id")) {
                        outcome.setActionId(payload.get("action_id").asText());
                    }
                    if (payload.has("scheduled_for")) {
                        outcome.setScheduledFor(payload.get("scheduled_for").asText());
                    }
                }
                return outcome;
            case "QuotaExceeded":
                outcome.setType(OutcomeType.QUOTA_EXCEEDED);
                if (payload != null) {
                    if (payload.has("tenant")) {
                        outcome.setTenant(payload.get("tenant").asText());
                    }
                    if (payload.has("limit")) {
                        outcome.setQuotaLimit(payload.get("limit").asLong());
                    }
                    if (payload.has("used")) {
                        outcome.setQuotaUsed(payload.get("used").asLong());
                    }
                    if (payload.has("overage_behavior")) {
                        outcome.setOverageBehavior(payload.get("overage_behavior").asText());
                    }
                }
                return outcome;
            default:
                outcome.setType(OutcomeType.FAILED);
                outcome.setError(new ActionError("UNKNOWN", "Unknown outcome variant: " + variant, false, 0));
                return outcome;
        }
    }
}
