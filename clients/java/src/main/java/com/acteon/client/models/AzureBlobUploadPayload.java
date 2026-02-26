package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the Azure Blob Storage upload action ({@code azure-blob}, action type {@code upload_blob}).
 */
public class AzureBlobUploadPayload {
    private final String blobName;
    private String container;
    private String body;
    private String bodyBase64;
    private String contentType;
    private Map<String, String> metadata;

    public AzureBlobUploadPayload(String blobName) {
        this.blobName = blobName;
    }

    public AzureBlobUploadPayload withContainer(String container) {
        this.container = container;
        return this;
    }

    public AzureBlobUploadPayload withBody(String body) {
        this.body = body;
        return this;
    }

    public AzureBlobUploadPayload withBodyBase64(String bodyBase64) {
        this.bodyBase64 = bodyBase64;
        return this;
    }

    public AzureBlobUploadPayload withContentType(String contentType) {
        this.contentType = contentType;
        return this;
    }

    public AzureBlobUploadPayload withMetadata(Map<String, String> metadata) {
        this.metadata = metadata;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("blob_name", blobName);
        if (container != null) payload.put("container", container);
        if (body != null) payload.put("body", body);
        if (bodyBase64 != null) payload.put("body_base64", bodyBase64);
        if (contentType != null) payload.put("content_type", contentType);
        if (metadata != null) payload.put("metadata", metadata);
        return payload;
    }
}
