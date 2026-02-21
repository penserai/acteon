package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * An attachment with explicit metadata and base64-encoded data.
 */
public class Attachment {
    private String id;
    private String name;
    private String filename;
    @JsonProperty("content_type")
    private String contentType;
    @JsonProperty("data_base64")
    private String dataBase64;

    /** No-arg constructor for Jackson deserialization. */
    public Attachment() {}

    /**
     * Creates an attachment.
     *
     * @param id          user-set identifier for referencing in chains
     * @param name        human-readable display name
     * @param filename    filename with extension
     * @param contentType MIME type (e.g., "application/pdf")
     * @param dataBase64  base64-encoded file content
     */
    public Attachment(String id, String name, String filename, String contentType, String dataBase64) {
        this.id = id;
        this.name = name;
        this.filename = filename;
        this.contentType = contentType;
        this.dataBase64 = dataBase64;
    }

    public String getId() { return id; }
    public void setId(String id) { this.id = id; }

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public String getFilename() { return filename; }
    public void setFilename(String filename) { this.filename = filename; }

    public String getContentType() { return contentType; }
    public void setContentType(String contentType) { this.contentType = contentType; }

    public String getDataBase64() { return dataBase64; }
    public void setDataBase64(String dataBase64) { this.dataBase64 = dataBase64; }
}
