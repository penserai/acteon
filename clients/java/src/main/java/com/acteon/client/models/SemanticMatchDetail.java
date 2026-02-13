package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Details about a semantic match evaluation.
 *
 * <p>Present on trace entries where the rule uses a {@code SemanticMatch}
 * condition.  Contains the extracted text, the topic it was compared against,
 * the computed similarity score, and the configured threshold.</p>
 */
public class SemanticMatchDetail {
    @JsonProperty("extracted_text")
    private String extractedText;

    @JsonProperty("topic")
    private String topic;

    @JsonProperty("similarity")
    private double similarity;

    @JsonProperty("threshold")
    private double threshold;

    public String getExtractedText() { return extractedText; }
    public void setExtractedText(String extractedText) { this.extractedText = extractedText; }

    public String getTopic() { return topic; }
    public void setTopic(String topic) { this.topic = topic; }

    public double getSimilarity() { return similarity; }
    public void setSimilarity(double similarity) { this.similarity = similarity; }

    public double getThreshold() { return threshold; }
    public void setThreshold(double threshold) { this.threshold = threshold; }
}
