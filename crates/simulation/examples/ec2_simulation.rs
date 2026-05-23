//! Simulation demonstrating AWS EC2 and Auto Scaling provider action types.
//!
//! Covers all EC2 instance lifecycle operations (start, stop, reboot, terminate,
//! hibernate, run, describe) and EBS volume operations (attach, detach), plus
//! Auto Scaling Group management (describe, set capacity, update).
//!
//! Run with:
//! ```bash
//! cargo run -p acteon-simulation --example ec2_simulation
//! ```

use acteon_core::{Action, ActionOutcome};
use acteon_simulation::prelude::*;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("==================================================================");
    info!("       AWS EC2 & AUTO SCALING SIMULATION");
    info!("==================================================================\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("aws-ec2")
            .add_recording_provider("aws-autoscaling")
            .build(),
    )
    .await?;

    info!("Started simulation cluster with 1 node");
    info!("Registered providers: aws-ec2, aws-autoscaling\n");

    let mut results: Vec<(&str, ActionOutcome)> = Vec::new();

    // =========================================================================
    // 1. EC2 Start Instances
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  1. EC2 START INSTANCES");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "start_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a", "i-0def456abc789012b"]
        }),
    );

    info!("  Dispatching start_instances for 2 instances...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/start_instances", outcome));
    info!("");

    // =========================================================================
    // 2. EC2 Stop Instances (basic)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  2. EC2 STOP INSTANCES (basic)");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "stop_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a"]
        }),
    );

    info!("  Dispatching stop_instances (basic)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/stop_instances(basic)", outcome));
    info!("");

    // =========================================================================
    // 3. EC2 Stop Instances with hibernate
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  3. EC2 STOP INSTANCES (hibernate)");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "stop_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a"],
            "hibernate": true
        }),
    );

    info!("  Dispatching stop_instances with hibernate=true...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/stop_instances(hibernate)", outcome));
    info!("");

    // =========================================================================
    // 4. EC2 Stop Instances with force
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  4. EC2 STOP INSTANCES (force)");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "stop_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a"],
            "force": true
        }),
    );

    info!("  Dispatching stop_instances with force=true...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/stop_instances(force)", outcome));
    info!("");

    // =========================================================================
    // 5. EC2 Reboot Instances
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  5. EC2 REBOOT INSTANCES");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "reboot_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a"]
        }),
    );

    info!("  Dispatching reboot_instances...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/reboot_instances", outcome));
    info!("");

    // =========================================================================
    // 6. EC2 Terminate Instances
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  6. EC2 TERMINATE INSTANCES");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "terminate_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a", "i-0def456abc789012b"]
        }),
    );

    info!("  Dispatching terminate_instances for 2 instances...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/terminate_instances", outcome));
    info!("");

    // =========================================================================
    // 7. EC2 Hibernate Instances (sugar action type)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  7. EC2 HIBERNATE INSTANCES (sugar)");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "hibernate_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a"]
        }),
    );

    info!("  Dispatching hibernate_instances (sugar for stop+hibernate)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/hibernate_instances", outcome));
    info!("");

    // =========================================================================
    // 8. EC2 Run Instances (minimal)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  8. EC2 RUN INSTANCES (minimal)");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "run_instances",
        serde_json::json!({
            "image_id": "ami-0abcdef1234567890",
            "instance_type": "t3.nano"
        }),
    );

    info!("  Dispatching run_instances (minimal payload)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/run_instances(minimal)", outcome));
    info!("");

    // =========================================================================
    // 9. EC2 Run Instances (full options)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  9. EC2 RUN INSTANCES (full options)");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "run_instances",
        serde_json::json!({
            "image_id": "ami-0abcdef1234567890",
            "instance_type": "t3.micro",
            "min_count": 2,
            "max_count": 5,
            "key_name": "deploy-key",
            "security_group_ids": ["sg-0123456789abcdef0", "sg-0abcdef0123456789"],
            "subnet_id": "subnet-0123456789abcdef0",
            "user_data": "IyEvYmluL2Jhc2gKZWNobyBIZWxsbw==",
            "tags": {
                "env": "staging",
                "team": "platform",
                "project": "acteon"
            },
            "iam_instance_profile": "arn:aws:iam::123456789012:instance-profile/app-profile"
        }),
    );

    info!("  Dispatching run_instances (full options with tags, SGs, user data)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/run_instances(full)", outcome));
    info!("");

    // =========================================================================
    // 10. EC2 Attach Volume
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  10. EC2 ATTACH VOLUME");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "attach_volume",
        serde_json::json!({
            "volume_id": "vol-0abc123def456789a",
            "instance_id": "i-0abc123def456789a",
            "device": "/dev/sdf"
        }),
    );

    info!("  Dispatching attach_volume...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/attach_volume", outcome));
    info!("");

    // =========================================================================
    // 11. EC2 Detach Volume
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  11. EC2 DETACH VOLUME");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "detach_volume",
        serde_json::json!({
            "volume_id": "vol-0abc123def456789a",
            "instance_id": "i-0abc123def456789a",
            "device": "/dev/sdf"
        }),
    );

    info!("  Dispatching detach_volume...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/detach_volume", outcome));
    info!("");

    // =========================================================================
    // 12. EC2 Detach Volume with force
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  12. EC2 DETACH VOLUME (force)");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "detach_volume",
        serde_json::json!({
            "volume_id": "vol-0abc123def456789a",
            "force": true
        }),
    );

    info!("  Dispatching detach_volume with force=true...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/detach_volume(force)", outcome));
    info!("");

    // =========================================================================
    // 13. EC2 Describe Instances
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  13. EC2 DESCRIBE INSTANCES");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "describe_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a"]
        }),
    );

    info!("  Dispatching describe_instances...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/describe_instances", outcome));
    info!("");

    // =========================================================================
    // 14. ASG Describe Auto Scaling Groups
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  14. ASG DESCRIBE AUTO SCALING GROUPS");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "scaling",
        "acme-corp",
        "aws-autoscaling",
        "describe_auto_scaling_groups",
        serde_json::json!({
            "auto_scaling_group_names": ["web-tier-asg", "api-tier-asg"]
        }),
    );

    info!("  Dispatching describe_auto_scaling_groups...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("asg/describe_groups", outcome));
    info!("");

    // =========================================================================
    // 15. ASG Set Desired Capacity
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  15. ASG SET DESIRED CAPACITY");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "scaling",
        "acme-corp",
        "aws-autoscaling",
        "set_desired_capacity",
        serde_json::json!({
            "auto_scaling_group_name": "web-tier-asg",
            "desired_capacity": 6,
            "honor_cooldown": true
        }),
    );

    info!("  Dispatching set_desired_capacity (desired=6, honor_cooldown)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("asg/set_desired_capacity", outcome));
    info!("");

    // =========================================================================
    // 16. ASG Update Auto Scaling Group
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  16. ASG UPDATE AUTO SCALING GROUP");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "scaling",
        "acme-corp",
        "aws-autoscaling",
        "update_auto_scaling_group",
        serde_json::json!({
            "auto_scaling_group_name": "web-tier-asg",
            "min_size": 2,
            "max_size": 20,
            "desired_capacity": 8,
            "default_cooldown": 300,
            "health_check_type": "ELB",
            "health_check_grace_period": 120
        }),
    );

    info!("  Dispatching update_auto_scaling_group (full update)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("asg/update_group", outcome));
    info!("");

    // =========================================================================
    // 17. EC2 Unknown Action Type (routed to recording provider)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  17. EC2 UNKNOWN ACTION TYPE");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "create_snapshot",
        serde_json::json!({
            "volume_id": "vol-0abc123def456789a"
        }),
    );

    info!("  Dispatching unknown EC2 action type 'create_snapshot'...");
    info!(
        "  (Recording provider accepts all action types; real Ec2Provider rejects unknown types)"
    );
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("ec2/unknown_action", outcome));
    info!("");

    // =========================================================================
    // 18. ASG Unknown Action Type (routed to recording provider)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  18. ASG UNKNOWN ACTION TYPE");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "scaling",
        "acme-corp",
        "aws-autoscaling",
        "delete_auto_scaling_group",
        serde_json::json!({
            "auto_scaling_group_name": "old-asg"
        }),
    );

    info!("  Dispatching unknown ASG action type 'delete_auto_scaling_group'...");
    info!(
        "  (Recording provider accepts all action types; real AutoScalingProvider rejects unknown types)"
    );
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("asg/unknown_action", outcome));
    info!("");

    // =========================================================================
    // Summary
    // =========================================================================
    info!("==================================================================");
    info!("  SUMMARY");
    info!("==================================================================\n");

    let mut all_passed = true;
    for (name, outcome) in &results {
        let passed = matches!(outcome, ActionOutcome::Executed(_));
        let status = if passed { "PASS" } else { "FAIL" };
        info!("  [{status}] {name}: {outcome:?}");
        if !passed {
            all_passed = false;
        }
    }

    info!("");
    info!(
        "  Total dispatched: {}  |  EC2 calls: {}  |  ASG calls: {}",
        results.len(),
        harness.provider("aws-ec2").unwrap().call_count(),
        harness.provider("aws-autoscaling").unwrap().call_count(),
    );

    harness.teardown().await?;
    info!("\n  Simulation cluster shut down");

    if all_passed {
        info!("\n  All EC2 and Auto Scaling actions dispatched successfully.");
    } else {
        info!("\n  Some actions failed. See details above.");
        std::process::exit(1);
    }

    Ok(())
}
