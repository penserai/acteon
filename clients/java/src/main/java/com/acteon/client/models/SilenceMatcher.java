package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * A single label matcher within a silence. Matchers in a silence are
 * AND-ed together. Regex patterns are capped at 256 characters and a
 * 64 KB compiled DFA server-side to prevent ReDoS.
 */
public class SilenceMatcher {
    @JsonProperty("name")
    private String name;

    @JsonProperty("value")
    private String value;

    /** One of: "equal", "not_equal", "regex", "not_regex". */
    @JsonProperty("op")
    private String op = "equal";

    public SilenceMatcher() {}

    public SilenceMatcher(String name, String value, String op) {
        this.name = name;
        this.value = value;
        this.op = op;
    }

    public static SilenceMatcher equal(String name, String value) {
        return new SilenceMatcher(name, value, "equal");
    }

    public static SilenceMatcher notEqual(String name, String value) {
        return new SilenceMatcher(name, value, "not_equal");
    }

    public static SilenceMatcher regex(String name, String value) {
        return new SilenceMatcher(name, value, "regex");
    }

    public static SilenceMatcher notRegex(String name, String value) {
        return new SilenceMatcher(name, value, "not_regex");
    }

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public String getValue() { return value; }
    public void setValue(String value) { this.value = value; }

    public String getOp() { return op; }
    public void setOp(String op) { this.op = op; }
}
