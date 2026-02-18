package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the AWS S3 get-object action ({@code aws-s3}, action type {@code get_object}).
 */
public class S3GetObjectPayload {
    private final String key;
    private String bucket;

    public S3GetObjectPayload(String key) {
        this.key = key;
    }

    public S3GetObjectPayload withBucket(String bucket) {
        this.bucket = bucket;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("key", key);
        if (bucket != null) payload.put("bucket", bucket);
        return payload;
    }
}
