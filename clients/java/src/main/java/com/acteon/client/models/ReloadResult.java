package com.acteon.client.models;

import java.util.List;

/**
 * Result of reloading rules.
 */
public class ReloadResult {
    private int loaded;
    private List<String> errors;

    public int getLoaded() { return loaded; }
    public void setLoaded(int loaded) { this.loaded = loaded; }

    public List<String> getErrors() { return errors; }
    public void setErrors(List<String> errors) { this.errors = errors; }
}
