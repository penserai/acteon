package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the AWS S3 put-object action ({@code aws-s3}, action type {@code put_object}).
 */
public class S3PutObjectPayload {
    private final String key;
    private String bucket;
    private String body;
    private String bodyBase64;
    private String contentType;
    private Map<String, String> metadata;

    public S3PutObjectPayload(String key) {
        this.key = key;
    }

    public S3PutObjectPayload withBucket(String bucket) {
        this.bucket = bucket;
        return this;
    }

    public S3PutObjectPayload withBody(String body) {
        this.body = body;
        return this;
    }

    public S3PutObjectPayload withBodyBase64(String bodyBase64) {
        this.bodyBase64 = bodyBase64;
        return this;
    }

    public S3PutObjectPayload withContentType(String contentType) {
        this.contentType = contentType;
        return this;
    }

    public S3PutObjectPayload withMetadata(Map<String, String> metadata) {
        this.metadata = metadata;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("key", key);
        if (bucket != null) payload.put("bucket", bucket);
        if (body != null) payload.put("body", body);
        if (bodyBase64 != null) payload.put("body_base64", bodyBase64);
        if (contentType != null) payload.put("content_type", contentType);
        if (metadata != null) payload.put("metadata", metadata);
        return payload;
    }
}
