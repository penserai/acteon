package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/** Explicit issued permit reference; the server validates current authority. */
public record PermitReference(String id, @JsonProperty("accepted_revision") long acceptedRevision) {}
