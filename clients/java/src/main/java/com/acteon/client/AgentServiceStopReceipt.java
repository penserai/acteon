package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;

/** A cancellation acknowledgement with separate provider finality. */
public record AgentServiceStopReceipt(
        JsonNode task,
        boolean futureStartsBlocked,
        boolean cancellationConfirmed,
        AgentServiceProviderAbort providerAbort) {}
