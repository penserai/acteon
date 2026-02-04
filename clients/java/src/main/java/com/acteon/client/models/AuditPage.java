package com.acteon.client.models;

import java.util.List;

/**
 * Paginated audit results.
 */
public class AuditPage {
    private List<AuditRecord> records;
    private long total;
    private long limit;
    private long offset;

    public List<AuditRecord> getRecords() { return records; }
    public void setRecords(List<AuditRecord> records) { this.records = records; }

    public long getTotal() { return total; }
    public void setTotal(long total) { this.total = total; }

    public long getLimit() { return limit; }
    public void setLimit(long limit) { this.limit = limit; }

    public long getOffset() { return offset; }
    public void setOffset(long offset) { this.offset = offset; }
}
