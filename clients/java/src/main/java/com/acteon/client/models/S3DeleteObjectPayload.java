package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the AWS S3 delete-object action ({@code aws-s3}, action type {@code delete_object}).
 */
public class S3DeleteObjectPayload {
    private final String key;
    private String bucket;

    public S3DeleteObjectPayload(String key) {
        this.key = key;
    }

    public S3DeleteObjectPayload withBucket(String bucket) {
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
