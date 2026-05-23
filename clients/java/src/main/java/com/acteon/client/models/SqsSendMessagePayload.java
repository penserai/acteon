package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the AWS SQS provider ({@code aws-sqs}, action type {@code send_message}).
 */
public class SqsSendMessagePayload {
    private final String messageBody;
    private String queueUrl;
    private Integer delaySeconds;
    private String messageGroupId;
    private String messageDedupId;
    private Map<String, String> messageAttributes;

    public SqsSendMessagePayload(String messageBody) {
        this.messageBody = messageBody;
    }

    public SqsSendMessagePayload withQueueUrl(String queueUrl) {
        this.queueUrl = queueUrl;
        return this;
    }

    public SqsSendMessagePayload withDelaySeconds(int delaySeconds) {
        this.delaySeconds = delaySeconds;
        return this;
    }

    public SqsSendMessagePayload withMessageGroupId(String messageGroupId) {
        this.messageGroupId = messageGroupId;
        return this;
    }

    public SqsSendMessagePayload withMessageDedupId(String messageDedupId) {
        this.messageDedupId = messageDedupId;
        return this;
    }

    public SqsSendMessagePayload withMessageAttributes(Map<String, String> messageAttributes) {
        this.messageAttributes = messageAttributes;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("message_body", messageBody);
        if (queueUrl != null) payload.put("queue_url", queueUrl);
        if (delaySeconds != null) payload.put("delay_seconds", delaySeconds);
        if (messageGroupId != null) payload.put("message_group_id", messageGroupId);
        if (messageDedupId != null) payload.put("message_dedup_id", messageDedupId);
        if (messageAttributes != null) payload.put("message_attributes", messageAttributes);
        return payload;
    }
}
