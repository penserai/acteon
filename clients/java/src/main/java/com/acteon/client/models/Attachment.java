package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * An attachment on an action, either a blob reference or inline data.
 *
 * <p>Use {@link #blobRef(String, String)} or {@link #inline(String, String, String)}
 * factory methods to create instances.</p>
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class Attachment {
    private String type;
    @JsonProperty("blob_id")
    private String blobId;
    @JsonProperty("data_base64")
    private String dataBase64;
    @JsonProperty("content_type")
    private String contentType;
    private String filename;

    public Attachment() {}

    /**
     * Creates a blob reference attachment.
     *
     * @param blobId   the unique identifier of the stored blob
     * @param filename optional filename (may be null)
     * @return a new blob reference attachment
     */
    public static Attachment blobRef(String blobId, String filename) {
        Attachment a = new Attachment();
        a.type = "blob_ref";
        a.blobId = blobId;
        a.filename = filename;
        return a;
    }

    /**
     * Creates an inline attachment with base64-encoded data.
     *
     * @param dataBase64  base64-encoded attachment data
     * @param contentType MIME type (e.g., "application/pdf")
     * @param filename    filename for the attachment
     * @return a new inline attachment
     */
    public static Attachment inline(String dataBase64, String contentType, String filename) {
        Attachment a = new Attachment();
        a.type = "inline";
        a.dataBase64 = dataBase64;
        a.contentType = contentType;
        a.filename = filename;
        return a;
    }

    public String getType() { return type; }
    public void setType(String type) { this.type = type; }

    public String getBlobId() { return blobId; }
    public void setBlobId(String blobId) { this.blobId = blobId; }

    public String getDataBase64() { return dataBase64; }
    public void setDataBase64(String dataBase64) { this.dataBase64 = dataBase64; }

    public String getContentType() { return contentType; }
    public void setContentType(String contentType) { this.contentType = contentType; }

    public String getFilename() { return filename; }
    public void setFilename(String filename) { this.filename = filename; }
}
