package com.acteon.client.models.deser;

import com.acteon.client.JsonMapper;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import java.time.Duration;

import static org.junit.jupiter.api.Assertions.*;

class RustDurationDeserializerTest {
    private static final ObjectMapper MAPPER = JsonMapper.build();

    @Test
    void decodesSecsAndNanos() throws Exception {
        Duration d = MAPPER.readValue("{\"secs\": 5, \"nanos\": 500000000}", Duration.class);

        assertEquals(5_500_000_000L, d.toNanos());
    }

    @Test
    void zeroValuesProduceZeroDuration() throws Exception {
        Duration d = MAPPER.readValue("{\"secs\": 0, \"nanos\": 0}", Duration.class);

        assertEquals(Duration.ZERO, d);
    }

    @Test
    void missingNanosDefaultsToZero() throws Exception {
        Duration d = MAPPER.readValue("{\"secs\": 7}", Duration.class);

        assertEquals(Duration.ofSeconds(7), d);
    }

    @Test
    void rawNumberFallsBackToSeconds() throws Exception {
        // Defensive — a server that ever serialized Duration as a
        // bare integer would still decode rather than blow up.
        Duration d = MAPPER.readValue("42", Duration.class);

        assertEquals(Duration.ofSeconds(42), d);
    }
}
