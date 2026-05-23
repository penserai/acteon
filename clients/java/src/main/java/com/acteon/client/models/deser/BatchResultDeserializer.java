package com.acteon.client.models.deser;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.databind.DeserializationContext;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.deser.std.StdDeserializer;

import com.acteon.client.models.ActionOutcome;
import com.acteon.client.models.BatchResult;
import com.acteon.client.models.ErrorResponse;

import java.io.IOException;

/**
 * Decodes a {@link BatchResult} from the server's batch dispatch
 * response. Each element of the batch response is either:
 *
 * <ul>
 *   <li>{@code {"error": {...}}} — the action could not be parsed
 *       or rejected by the gateway before reaching a provider</li>
 *   <li>An {@link ActionOutcome} shape (e.g.
 *       {@code {"Executed": {...}}} or {@code "Deduplicated"}) —
 *       the action made it through the pipeline and produced an
 *       outcome</li>
 * </ul>
 *
 * <p>Jackson can't pick between those two shapes via standard
 * polymorphism because the action-outcome variant uses Rust's
 * adjacent-tagged enum encoding, not a discriminator field.
 */
public class BatchResultDeserializer extends StdDeserializer<BatchResult> {
    public BatchResultDeserializer() {
        super(BatchResult.class);
    }

    @Override
    public BatchResult deserialize(JsonParser p, DeserializationContext ctxt) throws IOException {
        JsonNode node = p.getCodec().readTree(p);
        ObjectMapper mapper = (ObjectMapper) p.getCodec();
        BatchResult result = new BatchResult();

        if (node.isObject() && node.has("error") && node.size() == 1) {
            result.setSuccess(false);
            result.setError(mapper.treeToValue(node.get("error"), ErrorResponse.class));
            return result;
        }

        result.setSuccess(true);
        result.setOutcome(mapper.treeToValue(node, ActionOutcome.class));
        return result;
    }
}
