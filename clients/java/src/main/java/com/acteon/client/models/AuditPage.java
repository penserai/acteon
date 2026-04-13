package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.List;

/**
 * Paginated audit results.
 *
 * <p>{@code total} is {@code null} when the backend skipped the count
 * (always the case when paginating with a cursor). {@code nextCursor}
 * is {@code null} when this page is the last; otherwise pass it back
 * into {@link AuditQuery#setCursor(String)} to resume.
 */
public class AuditPage {
    private List<AuditRecord> records;
    private Long total;
    private long limit;
    private long offset;

    @JsonProperty("next_cursor")
    private String nextCursor;

    public List<AuditRecord> getRecords() { return records; }
    public void setRecords(List<AuditRecord> records) { this.records = records; }

    public Long getTotal() { return total; }
    public void setTotal(Long total) { this.total = total; }

    public long getLimit() { return limit; }
    public void setLimit(long limit) { this.limit = limit; }

    public long getOffset() { return offset; }
    public void setOffset(long offset) { this.offset = offset; }

    public String getNextCursor() { return nextCursor; }
    public void setNextCursor(String nextCursor) { this.nextCursor = nextCursor; }
}
