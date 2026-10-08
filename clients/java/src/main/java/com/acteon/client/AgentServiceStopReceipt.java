package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;

/** A future-start restriction acknowledgement with separate provider finality. */
public record AgentServiceStopReceipt(
        JsonNode task,
        boolean futureStartsBlocked,
        AgentServiceProviderAbort providerAbort) {}
