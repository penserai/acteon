package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the AWS SNS provider ({@code aws-sns}, action type {@code publish}).
 */
public class SnsPublishPayload {
    private final String message;
    private String subject;
    private String topicArn;
    private String messageGroupId;
    private String messageDedupId;

    public SnsPublishPayload(String message) {
        this.message = message;
    }

    public SnsPublishPayload withSubject(String subject) {
        this.subject = subject;
        return this;
    }

    public SnsPublishPayload withTopicArn(String topicArn) {
        this.topicArn = topicArn;
        return this;
    }

    public SnsPublishPayload withMessageGroupId(String messageGroupId) {
        this.messageGroupId = messageGroupId;
        return this;
    }

    public SnsPublishPayload withMessageDedupId(String messageDedupId) {
        this.messageDedupId = messageDedupId;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("message", message);
        if (subject != null) payload.put("subject", subject);
        if (topicArn != null) payload.put("topic_arn", topicArn);
        if (messageGroupId != null) payload.put("message_group_id", messageGroupId);
        if (messageDedupId != null) payload.put("message_dedup_id", messageDedupId);
        return payload;
    }
}
