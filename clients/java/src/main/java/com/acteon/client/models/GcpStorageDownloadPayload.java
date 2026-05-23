package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the GCP Cloud Storage download action ({@code gcp-storage}, action type {@code download_object}).
 */
public class GcpStorageDownloadPayload {
    private final String objectName;
    private String bucket;

    public GcpStorageDownloadPayload(String objectName) {
        this.objectName = objectName;
    }

    public GcpStorageDownloadPayload withBucket(String bucket) {
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
