package com.acteon.client.models;

import com.fasterxml.jackson.databind.ObjectMapper;
import java.nio.file.Files;
import java.nio.file.Path;
import static org.junit.jupiter.api.Assertions.assertEquals;
import org.junit.jupiter.api.Test;

class ActionWireTest {
    @Test void metadataMatchesRustWireShape() throws Exception {
        var mapper = new ObjectMapper();
        var fixture = mapper.readTree(Files.readString(Path.of("../contract-fixtures/action-wire.json")));
        var metadata = mapper.treeToValue(fixture.get("metadata"), ActionMetadata.class);
        assertEquals("alice", metadata.getLabels().get("owner"));
        assertEquals(fixture.get("metadata"), mapper.valueToTree(metadata));
    }
}
