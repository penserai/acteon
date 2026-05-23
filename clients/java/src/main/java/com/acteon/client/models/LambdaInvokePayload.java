package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the AWS Lambda provider ({@code aws-lambda}, action type {@code invoke}).
 */
public class LambdaInvokePayload {
    private Object payloadData;
    private String functionName;
    private String invocationType;

    public LambdaInvokePayload() {}

    public LambdaInvokePayload(Object payloadData) {
        this.payloadData = payloadData;
    }

    public LambdaInvokePayload withPayload(Object payloadData) {
        this.payloadData = payloadData;
        return this;
    }

    public LambdaInvokePayload withFunctionName(String functionName) {
        this.functionName = functionName;
        return this;
    }

    public LambdaInvokePayload withInvocationType(String invocationType) {
        this.invocationType = invocationType;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        if (payloadData != null) payload.put("payload", payloadData);
        if (functionName != null) payload.put("function_name", functionName);
        if (invocationType != null) payload.put("invocation_type", invocationType);
        return payload;
    }
}
