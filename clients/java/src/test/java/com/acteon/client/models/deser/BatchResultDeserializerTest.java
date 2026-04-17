package com.acteon.client.models.deser;

import com.acteon.client.JsonMapper;
import com.acteon.client.models.ActionOutcome.OutcomeType;
import com.acteon.client.models.BatchResult;
import com.fasterxml.jackson.core.type.TypeReference;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class BatchResultDeserializerTest {
    private static final ObjectMapper MAPPER = JsonMapper.build();

    @Test
    void errorVariantSetsErrorAndUnsetsOutcome() throws Exception {
        String json = """
            {"error": {"code": "BAD_REQUEST", "message": "missing field", "retryable": false}}
            """;

        BatchResult result = MAPPER.readValue(json, BatchResult.class);

        assertFalse(result.isSuccess());
        assertNull(result.getOutcome());
        assertNotNull(result.getError());
        assertEquals("BAD_REQUEST", result.getError().getCode());
    }

    @Test
    void actionOutcomeVariantSetsOutcome() throws Exception {
        String json = """
            {"Executed": {"status": "success", "body": {}}}
            """;

        BatchResult result = MAPPER.readValue(json, BatchResult.class);

        assertTrue(result.isSuccess());
        assertNull(result.getError());
        assertNotNull(result.getOutcome());
        assertEquals(OutcomeType.EXECUTED, result.getOutcome().getType());
    }

    @Test
    void deduplicatedBareStringIsSuccessfulOutcome() throws Exception {
        BatchResult result = MAPPER.readValue("\"Deduplicated\"", BatchResult.class);

        assertTrue(result.isSuccess());
        assertNull(result.getError());
        assertEquals(OutcomeType.DEDUPLICATED, result.getOutcome().getType());
    }

    @Test
    void roundTripsBatchListAcrossMixedShapes() throws Exception {
        // Mirrors the dispatchBatch / dispatchBatchDryRun call sites
        // in ActeonClient — server returns a list of mixed batch
        // results; client decodes via List<BatchResult>.
        String json = """
            [
              {"Executed": {"status": "success", "body": {}}},
              {"error": {"code": "RATE_LIMITED", "message": "slow down", "retryable": true}},
              "Deduplicated"
            ]
            """;

        List<BatchResult> results = MAPPER.readValue(json, new TypeReference<List<BatchResult>>() {});

        assertEquals(3, results.size());
        assertEquals(OutcomeType.EXECUTED, results.get(0).getOutcome().getType());
        assertEquals("RATE_LIMITED", results.get(1).getError().getCode());
        assertEquals(OutcomeType.DEDUPLICATED, results.get(2).getOutcome().getType());
    }
}
