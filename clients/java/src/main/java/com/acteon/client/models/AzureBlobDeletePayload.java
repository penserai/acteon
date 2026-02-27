package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the Azure Blob Storage delete action ({@code azure-blob}, action type {@code delete_blob}).
 */
public class AzureBlobDeletePayload {
    private final String blobName;
    private String container;

    public AzureBlobDeletePayload(String blobName) {
        this.blobName = blobName;
    }

    public AzureBlobDeletePayload withContainer(String container) {
        this.container = container;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("blob_name", blobName);
        if (container != null) payload.put("container", container);
        return payload;
    }
}
