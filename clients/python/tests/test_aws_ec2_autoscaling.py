"""Tests for AWS EC2 and Auto Scaling payload helpers in acteon_client.models."""

import unittest
from acteon_client.models import (
    ec2_start_instances_payload,
    ec2_stop_instances_payload,
    ec2_reboot_instances_payload,
    ec2_terminate_instances_payload,
    ec2_hibernate_instances_payload,
    ec2_run_instances_payload,
    ec2_attach_volume_payload,
    ec2_detach_volume_payload,
    ec2_describe_instances_payload,
    autoscaling_describe_groups_payload,
    autoscaling_set_desired_capacity_payload,
    autoscaling_update_group_payload,
)


class TestEc2StartInstancesPayload(unittest.TestCase):
    """Tests for ec2_start_instances_payload."""

    def test_basic(self):
        result = ec2_start_instances_payload(["i-abc123", "i-def456"])
        self.assertEqual(result, {"instance_ids": ["i-abc123", "i-def456"]})

    def test_single_instance(self):
        result = ec2_start_instances_payload(["i-abc123"])
        self.assertEqual(result, {"instance_ids": ["i-abc123"]})


class TestEc2StopInstancesPayload(unittest.TestCase):
    """Tests for ec2_stop_instances_payload."""

    def test_basic(self):
        result = ec2_stop_instances_payload(["i-abc123"])
        self.assertEqual(result, {"instance_ids": ["i-abc123"]})
        self.assertNotIn("hibernate", result)
        self.assertNotIn("force", result)

    def test_with_hibernate(self):
        result = ec2_stop_instances_payload(["i-abc123"], hibernate=True)
        self.assertEqual(result["instance_ids"], ["i-abc123"])
        self.assertTrue(result["hibernate"])

    def test_with_force(self):
        result = ec2_stop_instances_payload(["i-abc123"], force=True)
        self.assertTrue(result["force"])

    def test_with_all_options(self):
        result = ec2_stop_instances_payload(
            ["i-abc123", "i-def456"],
            hibernate=True,
            force=True,
        )
        self.assertEqual(result["instance_ids"], ["i-abc123", "i-def456"])
        self.assertTrue(result["hibernate"])
        self.assertTrue(result["force"])


class TestEc2RebootInstancesPayload(unittest.TestCase):
    """Tests for ec2_reboot_instances_payload."""

    def test_basic(self):
        result = ec2_reboot_instances_payload(["i-abc123"])
        self.assertEqual(result, {"instance_ids": ["i-abc123"]})


class TestEc2TerminateInstancesPayload(unittest.TestCase):
    """Tests for ec2_terminate_instances_payload."""

    def test_basic(self):
        result = ec2_terminate_instances_payload(["i-abc123", "i-def456"])
        self.assertEqual(result, {"instance_ids": ["i-abc123", "i-def456"]})


class TestEc2HibernateInstancesPayload(unittest.TestCase):
    """Tests for ec2_hibernate_instances_payload."""

    def test_basic(self):
        result = ec2_hibernate_instances_payload(["i-abc123"])
        self.assertEqual(result, {"instance_ids": ["i-abc123"]})


class TestEc2RunInstancesPayload(unittest.TestCase):
    """Tests for ec2_run_instances_payload."""

    def test_basic(self):
        result = ec2_run_instances_payload("ami-12345678", "t3.micro")
        self.assertEqual(result, {
            "image_id": "ami-12345678",
            "instance_type": "t3.micro",
        })

    def test_with_all_options(self):
        result = ec2_run_instances_payload(
            "ami-12345678",
            "t3.large",
            min_count=2,
            max_count=5,
            key_name="my-keypair",
            security_group_ids=["sg-111", "sg-222"],
            subnet_id="subnet-abc",
            user_data="IyEvYmluL2Jhc2g=",
            tags={"Name": "web-server", "env": "staging"},
            iam_instance_profile="arn:aws:iam::123456789012:instance-profile/role",
        )
        self.assertEqual(result["image_id"], "ami-12345678")
        self.assertEqual(result["instance_type"], "t3.large")
        self.assertEqual(result["min_count"], 2)
        self.assertEqual(result["max_count"], 5)
        self.assertEqual(result["key_name"], "my-keypair")
        self.assertEqual(result["security_group_ids"], ["sg-111", "sg-222"])
        self.assertEqual(result["subnet_id"], "subnet-abc")
        self.assertEqual(result["user_data"], "IyEvYmluL2Jhc2g=")
        self.assertEqual(result["tags"], {"Name": "web-server", "env": "staging"})
        self.assertEqual(
            result["iam_instance_profile"],
            "arn:aws:iam::123456789012:instance-profile/role",
        )

    def test_optional_fields_omitted(self):
        result = ec2_run_instances_payload("ami-12345678", "t3.micro")
        self.assertNotIn("min_count", result)
        self.assertNotIn("max_count", result)
        self.assertNotIn("key_name", result)
        self.assertNotIn("security_group_ids", result)
        self.assertNotIn("subnet_id", result)
        self.assertNotIn("user_data", result)
        self.assertNotIn("tags", result)
        self.assertNotIn("iam_instance_profile", result)


class TestEc2AttachVolumePayload(unittest.TestCase):
    """Tests for ec2_attach_volume_payload."""

    def test_basic(self):
        result = ec2_attach_volume_payload("vol-abc123", "i-def456", "/dev/sdf")
        self.assertEqual(result, {
            "volume_id": "vol-abc123",
            "instance_id": "i-def456",
            "device": "/dev/sdf",
        })


class TestEc2DetachVolumePayload(unittest.TestCase):
    """Tests for ec2_detach_volume_payload."""

    def test_basic(self):
        result = ec2_detach_volume_payload("vol-abc123")
        self.assertEqual(result, {"volume_id": "vol-abc123"})
        self.assertNotIn("instance_id", result)
        self.assertNotIn("device", result)
        self.assertNotIn("force", result)

    def test_with_all_options(self):
        result = ec2_detach_volume_payload(
            "vol-abc123",
            instance_id="i-def456",
            device="/dev/sdf",
            force=True,
        )
        self.assertEqual(result["volume_id"], "vol-abc123")
        self.assertEqual(result["instance_id"], "i-def456")
        self.assertEqual(result["device"], "/dev/sdf")
        self.assertTrue(result["force"])


class TestEc2DescribeInstancesPayload(unittest.TestCase):
    """Tests for ec2_describe_instances_payload."""

    def test_empty(self):
        result = ec2_describe_instances_payload()
        self.assertEqual(result, {})

    def test_with_instance_ids(self):
        result = ec2_describe_instances_payload(instance_ids=["i-abc123", "i-def456"])
        self.assertEqual(result, {"instance_ids": ["i-abc123", "i-def456"]})


class TestAutoscalingDescribeGroupsPayload(unittest.TestCase):
    """Tests for autoscaling_describe_groups_payload."""

    def test_empty(self):
        result = autoscaling_describe_groups_payload()
        self.assertEqual(result, {})

    def test_with_group_names(self):
        result = autoscaling_describe_groups_payload(
            group_names=["my-asg-1", "my-asg-2"]
        )
        self.assertEqual(
            result,
            {"auto_scaling_group_names": ["my-asg-1", "my-asg-2"]},
        )


class TestAutoscalingSetDesiredCapacityPayload(unittest.TestCase):
    """Tests for autoscaling_set_desired_capacity_payload."""

    def test_basic(self):
        result = autoscaling_set_desired_capacity_payload("my-asg", 5)
        self.assertEqual(result, {
            "auto_scaling_group_name": "my-asg",
            "desired_capacity": 5,
        })
        self.assertNotIn("honor_cooldown", result)

    def test_with_honor_cooldown(self):
        result = autoscaling_set_desired_capacity_payload(
            "my-asg", 10, honor_cooldown=True
        )
        self.assertEqual(result["auto_scaling_group_name"], "my-asg")
        self.assertEqual(result["desired_capacity"], 10)
        self.assertTrue(result["honor_cooldown"])


class TestAutoscalingUpdateGroupPayload(unittest.TestCase):
    """Tests for autoscaling_update_group_payload."""

    def test_basic(self):
        result = autoscaling_update_group_payload("my-asg")
        self.assertEqual(result, {"auto_scaling_group_name": "my-asg"})

    def test_with_all_options(self):
        result = autoscaling_update_group_payload(
            "my-asg",
            min_size=1,
            max_size=10,
            desired_capacity=5,
            default_cooldown=300,
            health_check_type="ELB",
            health_check_grace_period=120,
        )
        self.assertEqual(result["auto_scaling_group_name"], "my-asg")
        self.assertEqual(result["min_size"], 1)
        self.assertEqual(result["max_size"], 10)
        self.assertEqual(result["desired_capacity"], 5)
        self.assertEqual(result["default_cooldown"], 300)
        self.assertEqual(result["health_check_type"], "ELB")
        self.assertEqual(result["health_check_grace_period"], 120)

    def test_optional_fields_omitted(self):
        result = autoscaling_update_group_payload("my-asg")
        self.assertNotIn("min_size", result)
        self.assertNotIn("max_size", result)
        self.assertNotIn("desired_capacity", result)
        self.assertNotIn("default_cooldown", result)
        self.assertNotIn("health_check_type", result)
        self.assertNotIn("health_check_grace_period", result)


if __name__ == "__main__":
    unittest.main()
