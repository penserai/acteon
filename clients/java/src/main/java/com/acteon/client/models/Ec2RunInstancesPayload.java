package com.acteon.client.models;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Payload builder for the AWS EC2 run-instances action ({@code aws-ec2}, action type {@code run_instances}).
 */
public class Ec2RunInstancesPayload {
    private final String imageId;
    private final String instanceType;
    private Integer minCount;
    private Integer maxCount;
    private String keyName;
    private List<String> securityGroupIds;
    private String subnetId;
    private String userData;
    private Map<String, String> tags;
    private String iamInstanceProfile;

    public Ec2RunInstancesPayload(String imageId, String instanceType) {
        this.imageId = imageId;
        this.instanceType = instanceType;
    }

    public Ec2RunInstancesPayload withMinCount(int minCount) {
        this.minCount = minCount;
        return this;
    }

    public Ec2RunInstancesPayload withMaxCount(int maxCount) {
        this.maxCount = maxCount;
        return this;
    }

    public Ec2RunInstancesPayload withKeyName(String keyName) {
        this.keyName = keyName;
        return this;
    }

    public Ec2RunInstancesPayload withSecurityGroupIds(List<String> securityGroupIds) {
        this.securityGroupIds = securityGroupIds;
        return this;
    }

    public Ec2RunInstancesPayload withSubnetId(String subnetId) {
        this.subnetId = subnetId;
        return this;
    }

    public Ec2RunInstancesPayload withUserData(String userData) {
        this.userData = userData;
        return this;
    }

    public Ec2RunInstancesPayload withTags(Map<String, String> tags) {
        this.tags = tags;
        return this;
    }

    public Ec2RunInstancesPayload withIamInstanceProfile(String iamInstanceProfile) {
        this.iamInstanceProfile = iamInstanceProfile;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("image_id", imageId);
        payload.put("instance_type", instanceType);
        if (minCount != null) payload.put("min_count", minCount);
        if (maxCount != null) payload.put("max_count", maxCount);
        if (keyName != null) payload.put("key_name", keyName);
        if (securityGroupIds != null) payload.put("security_group_ids", securityGroupIds);
        if (subnetId != null) payload.put("subnet_id", subnetId);
        if (userData != null) payload.put("user_data", userData);
        if (tags != null) payload.put("tags", tags);
        if (iamInstanceProfile != null) payload.put("iam_instance_profile", iamInstanceProfile);
        return payload;
    }
}
