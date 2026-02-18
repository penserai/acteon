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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("       AWS EC2 & AUTO SCALING SIMULATION");
    println!("==================================================================\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("aws-ec2")
            .add_recording_provider("aws-autoscaling")
            .build(),
    )
    .await?;

    println!("Started simulation cluster with 1 node");
    println!("Registered providers: aws-ec2, aws-autoscaling\n");

    let mut results: Vec<(&str, ActionOutcome)> = Vec::new();

    // =========================================================================
    // 1. EC2 Start Instances
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  1. EC2 START INSTANCES");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "start_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a", "i-0def456abc789012b"]
        }),
    );

    println!("  Dispatching start_instances for 2 instances...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/start_instances", outcome));
    println!();

    // =========================================================================
    // 2. EC2 Stop Instances (basic)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  2. EC2 STOP INSTANCES (basic)");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "stop_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a"]
        }),
    );

    println!("  Dispatching stop_instances (basic)...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/stop_instances(basic)", outcome));
    println!();

    // =========================================================================
    // 3. EC2 Stop Instances with hibernate
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  3. EC2 STOP INSTANCES (hibernate)");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching stop_instances with hibernate=true...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/stop_instances(hibernate)", outcome));
    println!();

    // =========================================================================
    // 4. EC2 Stop Instances with force
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  4. EC2 STOP INSTANCES (force)");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching stop_instances with force=true...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/stop_instances(force)", outcome));
    println!();

    // =========================================================================
    // 5. EC2 Reboot Instances
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  5. EC2 REBOOT INSTANCES");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "reboot_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a"]
        }),
    );

    println!("  Dispatching reboot_instances...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/reboot_instances", outcome));
    println!();

    // =========================================================================
    // 6. EC2 Terminate Instances
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  6. EC2 TERMINATE INSTANCES");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "terminate_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a", "i-0def456abc789012b"]
        }),
    );

    println!("  Dispatching terminate_instances for 2 instances...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/terminate_instances", outcome));
    println!();

    // =========================================================================
    // 7. EC2 Hibernate Instances (sugar action type)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  7. EC2 HIBERNATE INSTANCES (sugar)");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "hibernate_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a"]
        }),
    );

    println!("  Dispatching hibernate_instances (sugar for stop+hibernate)...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/hibernate_instances", outcome));
    println!();

    // =========================================================================
    // 8. EC2 Run Instances (minimal)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  8. EC2 RUN INSTANCES (minimal)");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching run_instances (minimal payload)...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/run_instances(minimal)", outcome));
    println!();

    // =========================================================================
    // 9. EC2 Run Instances (full options)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  9. EC2 RUN INSTANCES (full options)");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching run_instances (full options with tags, SGs, user data)...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/run_instances(full)", outcome));
    println!();

    // =========================================================================
    // 10. EC2 Attach Volume
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  10. EC2 ATTACH VOLUME");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching attach_volume...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/attach_volume", outcome));
    println!();

    // =========================================================================
    // 11. EC2 Detach Volume
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  11. EC2 DETACH VOLUME");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching detach_volume...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/detach_volume", outcome));
    println!();

    // =========================================================================
    // 12. EC2 Detach Volume with force
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  12. EC2 DETACH VOLUME (force)");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching detach_volume with force=true...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/detach_volume(force)", outcome));
    println!();

    // =========================================================================
    // 13. EC2 Describe Instances
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  13. EC2 DESCRIBE INSTANCES");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "describe_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a"]
        }),
    );

    println!("  Dispatching describe_instances...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/describe_instances", outcome));
    println!();

    // =========================================================================
    // 14. ASG Describe Auto Scaling Groups
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  14. ASG DESCRIBE AUTO SCALING GROUPS");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "scaling",
        "acme-corp",
        "aws-autoscaling",
        "describe_auto_scaling_groups",
        serde_json::json!({
            "auto_scaling_group_names": ["web-tier-asg", "api-tier-asg"]
        }),
    );

    println!("  Dispatching describe_auto_scaling_groups...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("asg/describe_groups", outcome));
    println!();

    // =========================================================================
    // 15. ASG Set Desired Capacity
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  15. ASG SET DESIRED CAPACITY");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching set_desired_capacity (desired=6, honor_cooldown)...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("asg/set_desired_capacity", outcome));
    println!();

    // =========================================================================
    // 16. ASG Update Auto Scaling Group
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  16. ASG UPDATE AUTO SCALING GROUP");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching update_auto_scaling_group (full update)...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("asg/update_group", outcome));
    println!();

    // =========================================================================
    // 17. EC2 Unknown Action Type (routed to recording provider)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  17. EC2 UNKNOWN ACTION TYPE");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "compute",
        "acme-corp",
        "aws-ec2",
        "create_snapshot",
        serde_json::json!({
            "volume_id": "vol-0abc123def456789a"
        }),
    );

    println!("  Dispatching unknown EC2 action type 'create_snapshot'...");
    println!(
        "  (Recording provider accepts all action types; real Ec2Provider rejects unknown types)"
    );
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("ec2/unknown_action", outcome));
    println!();

    // =========================================================================
    // 18. ASG Unknown Action Type (routed to recording provider)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  18. ASG UNKNOWN ACTION TYPE");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "scaling",
        "acme-corp",
        "aws-autoscaling",
        "delete_auto_scaling_group",
        serde_json::json!({
            "auto_scaling_group_name": "old-asg"
        }),
    );

    println!("  Dispatching unknown ASG action type 'delete_auto_scaling_group'...");
    println!(
        "  (Recording provider accepts all action types; real AutoScalingProvider rejects unknown types)"
    );
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("asg/unknown_action", outcome));
    println!();

    // =========================================================================
    // Summary
    // =========================================================================
    println!("==================================================================");
    println!("  SUMMARY");
    println!("==================================================================\n");

    let mut all_passed = true;
    for (name, outcome) in &results {
        let passed = matches!(outcome, ActionOutcome::Executed(_));
        let status = if passed { "PASS" } else { "FAIL" };
        println!("  [{status}] {name}: {outcome:?}");
        if !passed {
            all_passed = false;
        }
    }

    println!();
    println!(
        "  Total dispatched: {}  |  EC2 calls: {}  |  ASG calls: {}",
        results.len(),
        harness.provider("aws-ec2").unwrap().call_count(),
        harness.provider("aws-autoscaling").unwrap().call_count(),
    );

    harness.teardown().await?;
    println!("\n  Simulation cluster shut down");

    if all_passed {
        println!("\n  All EC2 and Auto Scaling actions dispatched successfully.");
    } else {
        println!("\n  Some actions failed. See details above.");
        std::process::exit(1);
    }

    Ok(())
}
