package com.acteon.client.models;

import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

class Ec2AutoScalingPayloadTest {

    // =========================================================================
    // EC2 Start Instances
    // =========================================================================

    @Test
    void ec2StartInstancesBasic() {
        Map<String, Object> payload = new Ec2StartInstancesPayload(
                List.of("i-abc123", "i-def456")
        ).toPayload();

        assertEquals(List.of("i-abc123", "i-def456"), payload.get("instance_ids"));
    }

    // =========================================================================
    // EC2 Stop Instances
    // =========================================================================

    @Test
    void ec2StopInstancesBasic() {
        Map<String, Object> payload = new Ec2StopInstancesPayload(
                List.of("i-abc123")
        ).toPayload();

        assertEquals(List.of("i-abc123"), payload.get("instance_ids"));
        assertFalse(payload.containsKey("hibernate"));
        assertFalse(payload.containsKey("force"));
    }

    @Test
    void ec2StopInstancesWithOptions() {
        Map<String, Object> payload = new Ec2StopInstancesPayload(
                List.of("i-abc123", "i-def456")
        ).withHibernate(true).withForce(true).toPayload();

        assertEquals(List.of("i-abc123", "i-def456"), payload.get("instance_ids"));
        assertEquals(true, payload.get("hibernate"));
        assertEquals(true, payload.get("force"));
    }

    // =========================================================================
    // EC2 Run Instances
    // =========================================================================

    @Test
    void ec2RunInstancesBasic() {
        Map<String, Object> payload = new Ec2RunInstancesPayload(
                "ami-12345678", "t3.micro"
        ).toPayload();

        assertEquals("ami-12345678", payload.get("image_id"));
        assertEquals("t3.micro", payload.get("instance_type"));
        assertFalse(payload.containsKey("min_count"));
        assertFalse(payload.containsKey("key_name"));
        assertFalse(payload.containsKey("tags"));
    }

    @Test
    void ec2RunInstancesWithAllOptions() {
        Map<String, Object> payload = new Ec2RunInstancesPayload("ami-12345678", "t3.large")
                .withMinCount(2)
                .withMaxCount(5)
                .withKeyName("my-keypair")
                .withSecurityGroupIds(List.of("sg-111", "sg-222"))
                .withSubnetId("subnet-abc")
                .withUserData("IyEvYmluL2Jhc2g=")
                .withTags(Map.of("Name", "web-server", "env", "staging"))
                .withIamInstanceProfile("arn:aws:iam::123456789012:instance-profile/role")
                .toPayload();

        assertEquals("ami-12345678", payload.get("image_id"));
        assertEquals("t3.large", payload.get("instance_type"));
        assertEquals(2, payload.get("min_count"));
        assertEquals(5, payload.get("max_count"));
        assertEquals("my-keypair", payload.get("key_name"));
        assertEquals(List.of("sg-111", "sg-222"), payload.get("security_group_ids"));
        assertEquals("subnet-abc", payload.get("subnet_id"));
        assertEquals("IyEvYmluL2Jhc2g=", payload.get("user_data"));
        assertEquals("arn:aws:iam::123456789012:instance-profile/role", payload.get("iam_instance_profile"));

        @SuppressWarnings("unchecked")
        Map<String, String> tags = (Map<String, String>) payload.get("tags");
        assertEquals("web-server", tags.get("Name"));
        assertEquals("staging", tags.get("env"));
    }

    // =========================================================================
    // EC2 Attach Volume
    // =========================================================================

    @Test
    void ec2AttachVolumeBasic() {
        Map<String, Object> payload = new Ec2AttachVolumePayload(
                "vol-abc123", "i-def456", "/dev/sdf"
        ).toPayload();

        assertEquals("vol-abc123", payload.get("volume_id"));
        assertEquals("i-def456", payload.get("instance_id"));
        assertEquals("/dev/sdf", payload.get("device"));
    }

    // =========================================================================
    // EC2 Detach Volume
    // =========================================================================

    @Test
    void ec2DetachVolumeBasic() {
        Map<String, Object> payload = new Ec2DetachVolumePayload("vol-abc123").toPayload();

        assertEquals("vol-abc123", payload.get("volume_id"));
        assertFalse(payload.containsKey("instance_id"));
        assertFalse(payload.containsKey("device"));
        assertFalse(payload.containsKey("force"));
    }

    @Test
    void ec2DetachVolumeWithOptions() {
        Map<String, Object> payload = new Ec2DetachVolumePayload("vol-abc123")
                .withInstanceId("i-def456")
                .withDevice("/dev/sdf")
                .withForce(true)
                .toPayload();

        assertEquals("vol-abc123", payload.get("volume_id"));
        assertEquals("i-def456", payload.get("instance_id"));
        assertEquals("/dev/sdf", payload.get("device"));
        assertEquals(true, payload.get("force"));
    }

    // =========================================================================
    // EC2 Describe Instances
    // =========================================================================

    @Test
    void ec2DescribeInstancesEmpty() {
        Map<String, Object> payload = new Ec2DescribeInstancesPayload().toPayload();
        assertFalse(payload.containsKey("instance_ids"));
    }

    @Test
    void ec2DescribeInstancesWithIds() {
        Map<String, Object> payload = new Ec2DescribeInstancesPayload()
                .withInstanceIds(List.of("i-abc123", "i-def456"))
                .toPayload();

        assertEquals(List.of("i-abc123", "i-def456"), payload.get("instance_ids"));
    }

    // =========================================================================
    // Auto Scaling Describe Groups
    // =========================================================================

    @Test
    void asgDescribeGroupsEmpty() {
        Map<String, Object> payload = new AsgDescribeGroupsPayload().toPayload();
        assertFalse(payload.containsKey("auto_scaling_group_names"));
    }

    @Test
    void asgDescribeGroupsWithNames() {
        Map<String, Object> payload = new AsgDescribeGroupsPayload()
                .withGroupNames(List.of("my-asg-1", "my-asg-2"))
                .toPayload();

        assertEquals(List.of("my-asg-1", "my-asg-2"), payload.get("auto_scaling_group_names"));
    }

    // =========================================================================
    // Auto Scaling Set Desired Capacity
    // =========================================================================

    @Test
    void asgSetCapacityBasic() {
        Map<String, Object> payload = new AsgSetCapacityPayload("my-asg", 5).toPayload();

        assertEquals("my-asg", payload.get("auto_scaling_group_name"));
        assertEquals(5, payload.get("desired_capacity"));
        assertFalse(payload.containsKey("honor_cooldown"));
    }

    @Test
    void asgSetCapacityWithHonorCooldown() {
        Map<String, Object> payload = new AsgSetCapacityPayload("my-asg", 10)
                .withHonorCooldown(true)
                .toPayload();

        assertEquals("my-asg", payload.get("auto_scaling_group_name"));
        assertEquals(10, payload.get("desired_capacity"));
        assertEquals(true, payload.get("honor_cooldown"));
    }

    // =========================================================================
    // Auto Scaling Update Group
    // =========================================================================

    @Test
    void asgUpdateGroupBasic() {
        Map<String, Object> payload = new AsgUpdateGroupPayload("my-asg").toPayload();

        assertEquals("my-asg", payload.get("auto_scaling_group_name"));
        assertFalse(payload.containsKey("min_size"));
        assertFalse(payload.containsKey("max_size"));
    }

    @Test
    void asgUpdateGroupWithAllOptions() {
        Map<String, Object> payload = new AsgUpdateGroupPayload("my-asg")
                .withMinSize(1)
                .withMaxSize(10)
                .withDesiredCapacity(5)
                .withDefaultCooldown(300)
                .withHealthCheckType("ELB")
                .withHealthCheckGracePeriod(120)
                .toPayload();

        assertEquals("my-asg", payload.get("auto_scaling_group_name"));
        assertEquals(1, payload.get("min_size"));
        assertEquals(10, payload.get("max_size"));
        assertEquals(5, payload.get("desired_capacity"));
        assertEquals(300, payload.get("default_cooldown"));
        assertEquals("ELB", payload.get("health_check_type"));
        assertEquals(120, payload.get("health_check_grace_period"));
    }
}
