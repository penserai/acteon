use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{
    CreateProfileRequest, CreateTemplateRequest, RenderPreviewRequest, UpdateProfileRequest,
    UpdateTemplateRequest,
};
use clap::{Args, Subcommand};

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct TemplatesArgs {
    #[command(subcommand)]
    pub command: TemplatesCommand,
}

#[derive(Subcommand, Debug)]
pub enum TemplatesCommand {
    /// List templates.
    List {
        /// Filter by namespace.
        #[arg(long)]
        namespace: Option<String>,
        /// Filter by tenant.
        #[arg(long)]
        tenant: Option<String>,
    },
    /// Get a template by ID.
    Get {
        /// Template ID.
        id: String,
    },
    /// Create a template.
    Create {
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
    /// Update a template.
    Update {
        /// Template ID.
        id: String,
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
    /// Delete a template.
    Delete {
        /// Template ID.
        id: String,
    },
    /// Manage template profiles.
    Profiles(ProfilesArgs),
    /// Render a template preview.
    Render {
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
}

#[derive(Args, Debug)]
pub struct ProfilesArgs {
    #[command(subcommand)]
    pub command: ProfilesCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProfilesCommand {
    /// List template profiles.
    List {
        /// Filter by namespace.
        #[arg(long)]
        namespace: Option<String>,
        /// Filter by tenant.
        #[arg(long)]
        tenant: Option<String>,
    },
    /// Get a template profile by ID.
    Get {
        /// Profile ID.
        id: String,
    },
    /// Create a template profile.
    Create {
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
    /// Update a template profile.
    Update {
        /// Profile ID.
        id: String,
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
    /// Delete a template profile.
    Delete {
        /// Profile ID.
        id: String,
    },
}

fn parse_json_data(input: &str) -> anyhow::Result<serde_json::Value> {
    if let Some(path) = input.strip_prefix('@') {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    } else {
        Ok(serde_json::from_str(input)?)
    }
}

pub async fn run(
    ops: &OpsClient,
    args: &TemplatesArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        TemplatesCommand::List { namespace, tenant } => {
            let resp = ops
                .list_templates(namespace.as_deref(), tenant.as_deref())
                .await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("{} templates:", resp.count);
                    for t in &resp.templates {
                        let desc = t.description.as_deref().unwrap_or("");
                        println!(
                            "  {id} | {name} | {ns}:{tenant} {desc}",
                            id = &t.id[..8.min(t.id.len())],
                            name = t.name,
                            ns = t.namespace,
                            tenant = t.tenant,
                        );
                    }
                }
            }
        }
        TemplatesCommand::Get { id } => {
            let resp = ops.get_template(id).await?;
            match resp {
                Some(t) => match format {
                    OutputFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&t)?);
                    }
                    OutputFormat::Text => {
                        println!("ID:        {}", t.id);
                        println!("Name:      {}", t.name);
                        println!("Namespace: {}", t.namespace);
                        println!("Tenant:    {}", t.tenant);
                        if let Some(ref desc) = t.description {
                            println!("Desc:      {desc}");
                        }
                        println!("Content:\n{}", t.content);
                    }
                },
                None => {
                    println!("Template not found: {id}");
                }
            }
        }
        TemplatesCommand::Create { data } => {
            let value = parse_json_data(data)?;
            let req: CreateTemplateRequest = serde_json::from_value(value)?;
            let resp = ops.create_template(&req).await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("Created template: {} ({})", resp.id, resp.name);
                }
            }
        }
        TemplatesCommand::Update { id, data } => {
            let value = parse_json_data(data)?;
            let req: UpdateTemplateRequest = serde_json::from_value(value)?;
            let resp = ops.update_template(id, &req).await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("Updated template: {}", resp.id);
                }
            }
        }
        TemplatesCommand::Delete { id } => {
            ops.delete_template(id).await?;
            println!("Template '{id}' deleted.");
        }
        TemplatesCommand::Profiles(profiles_args) => {
            run_profiles(ops, profiles_args, format).await?;
        }
        TemplatesCommand::Render { data } => {
            let value = parse_json_data(data)?;
            let req: RenderPreviewRequest = serde_json::from_value(value)?;
            let resp = ops.render_preview(&req).await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("Rendered fields:");
                    for (field, content) in &resp.rendered {
                        println!("  {field}: {content}");
                    }
                }
            }
        }
    }
    Ok(())
}

async fn run_profiles(
    ops: &OpsClient,
    args: &ProfilesArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        ProfilesCommand::List { namespace, tenant } => {
            let resp = ops
                .list_profiles(namespace.as_deref(), tenant.as_deref())
                .await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("{} profiles:", resp.count);
                    for p in &resp.profiles {
                        let desc = p.description.as_deref().unwrap_or("");
                        println!(
                            "  {id} | {name} | {ns}:{tenant} | {fields} fields {desc}",
                            id = &p.id[..8.min(p.id.len())],
                            name = p.name,
                            ns = p.namespace,
                            tenant = p.tenant,
                            fields = p.fields.len(),
                        );
                    }
                }
            }
        }
        ProfilesCommand::Get { id } => {
            let resp = ops.get_profile(id).await?;
            match resp {
                Some(p) => match format {
                    OutputFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&p)?);
                    }
                    OutputFormat::Text => {
                        println!("ID:        {}", p.id);
                        println!("Name:      {}", p.name);
                        println!("Namespace: {}", p.namespace);
                        println!("Tenant:    {}", p.tenant);
                        println!("Fields:    {}", p.fields.len());
                        if let Some(ref desc) = p.description {
                            println!("Desc:      {desc}");
                        }
                    }
                },
                None => {
                    println!("Profile not found: {id}");
                }
            }
        }
        ProfilesCommand::Create { data } => {
            let value = parse_json_data(data)?;
            let req: CreateProfileRequest = serde_json::from_value(value)?;
            let resp = ops.create_profile(&req).await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("Created profile: {} ({})", resp.id, resp.name);
                }
            }
        }
        ProfilesCommand::Update { id, data } => {
            let value = parse_json_data(data)?;
            let req: UpdateProfileRequest = serde_json::from_value(value)?;
            let resp = ops.update_profile(id, &req).await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("Updated profile: {}", resp.id);
                }
            }
        }
        ProfilesCommand::Delete { id } => {
            ops.delete_profile(id).await?;
            println!("Profile '{id}' deleted.");
        }
    }
    Ok(())
}
