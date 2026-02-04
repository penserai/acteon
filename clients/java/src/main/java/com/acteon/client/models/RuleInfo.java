package com.acteon.client.models;

/**
 * Information about a loaded rule.
 */
public class RuleInfo {
    private String name;
    private int priority;
    private boolean enabled;
    private String description;

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public int getPriority() { return priority; }
    public void setPriority(int priority) { this.priority = priority; }

    public boolean isEnabled() { return enabled; }
    public void setEnabled(boolean enabled) { this.enabled = enabled; }

    public String getDescription() { return description; }
    public void setDescription(String description) { this.description = description; }
}
