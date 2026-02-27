package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the GCP Cloud Storage delete action ({@code gcp-storage}, action type {@code delete_object}).
 */
public class GcpStorageDeletePayload {
    private final String objectName;
    private String bucket;

    public GcpStorageDeletePayload(String objectName) {
        this.objectName = objectName;
    }

    public GcpStorageDeletePayload withBucket(String bucket) {
        this.bucket = bucket;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("object_name", objectName);
        if (bucket != null) payload.put("bucket", bucket);
        return payload;
    }
}
