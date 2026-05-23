package com.acteon.client;

import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.PropertyNamingStrategies;
import com.fasterxml.jackson.databind.module.SimpleModule;
import com.fasterxml.jackson.databind.SerializationFeature;
import com.fasterxml.jackson.annotation.JsonInclude;

import com.acteon.client.models.ActionOutcome;
import com.acteon.client.models.BatchResult;
import com.acteon.client.models.deser.ActionOutcomeDeserializer;
import com.acteon.client.models.deser.BatchResultDeserializer;
import com.acteon.client.models.deser.RustDurationDeserializer;

import java.time.Duration;

/**
 * Builds the {@link ObjectMapper} used by {@link ActeonClient} to
 * serialize requests and deserialize responses.
 *
 * <p>The mapper is configured to:
 * <ul>
 *   <li>Map Java {@code camelCase} fields to JSON {@code snake_case}
 *       via {@link PropertyNamingStrategies#SNAKE_CASE}, so model
 *       classes do not need a {@code @JsonProperty} on every field.
 *       Edge cases where the strategy mishandles a name (notably
 *       digit boundaries — {@code p50LatencyMs} would otherwise become
 *       {@code p_50_latency_ms}) carry an explicit
 *       {@code @JsonProperty} on that field only.</li>
 *   <li>Ignore unknown JSON properties so older clients keep working
 *       against newer servers that have grown new response fields.</li>
 *   <li>Skip {@code null} fields when serializing so optional request
 *       parameters stay out of the wire payload.</li>
 *   <li>Decode the polymorphic {@code ActionOutcome} (Rust serde
 *       adjacent-tagged enum: {@code {"Executed": {...}}} /
 *       {@code "Deduplicated"} bare string) and Rust's
 *       {@code Duration} ({@code {"secs": _, "nanos": _}}) shape via
 *       custom deserializers.</li>
 * </ul>
 */
public final class JsonMapper {
    private JsonMapper() {}

    public static ObjectMapper build() {
        ObjectMapper mapper = new ObjectMapper();
        mapper.setPropertyNamingStrategy(PropertyNamingStrategies.SNAKE_CASE);
        mapper.configure(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES, false);
        mapper.configure(SerializationFeature.WRITE_DATES_AS_TIMESTAMPS, false);
        mapper.setSerializationInclusion(JsonInclude.Include.NON_NULL);

        SimpleModule rustModule = new SimpleModule("RustInterop");
        rustModule.addDeserializer(ActionOutcome.class, new ActionOutcomeDeserializer());
        rustModule.addDeserializer(BatchResult.class, new BatchResultDeserializer());
        rustModule.addDeserializer(Duration.class, new RustDurationDeserializer());
        mapper.registerModule(rustModule);

        return mapper;
    }
}
