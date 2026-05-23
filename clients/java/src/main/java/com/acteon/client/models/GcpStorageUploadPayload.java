package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the GCP Cloud Storage upload action ({@code gcp-storage}, action type {@code upload_object}).
 */
public class GcpStorageUploadPayload {
    private final String objectName;
    private String bucket;
    private String body;
    private String bodyBase64;
    private String contentType;
    private Map<String, String> metadata;

    public GcpStorageUploadPayload(String objectName) {
        this.objectName = objectName;
    }

    public GcpStorageUploadPayload withBucket(String bucket) {
        this.bucket = bucket;
        return this;
    }

    public GcpStorageUploadPayload withBody(String body) {
        this.body = body;
        return this;
    }

    public GcpStorageUploadPayload withBodyBase64(String bodyBase64) {
        this.bodyBase64 = bodyBase64;
        return this;
    }

    public GcpStorageUploadPayload withContentType(String contentType) {
        this.contentType = contentType;
        return this;
    }

    public GcpStorageUploadPayload withMetadata(Map<String, String> metadata) {
        this.metadata = metadata;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("object_name", objectName);
        if (bucket != null) payload.put("bucket", bucket);
        if (body != null) payload.put("body", body);
        if (bodyBase64 != null) payload.put("body_base64", bodyBase64);
        if (contentType != null) payload.put("content_type", contentType);
        if (metadata != null) payload.put("metadata", metadata);
        return payload;
    }
}
