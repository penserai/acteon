package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;

/** A future-start restriction acknowledgement, not an external provider abort. */
public record AgentServiceStopReceipt(JsonNode task, boolean futureStartsBlocked) {}
