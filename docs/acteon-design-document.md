  
**ACTEON**

*Actions Forged in Rust*

Technical Design Document

Version 1.0

**Penserai**

github.com/penserai/acteon

# **Executive Summary**

Acteon is a distributed action gateway system built in Rust that executes actions across multiple providers (Slack, PagerDuty, Twilio, AWS SQS/SNS, LLM gateways, and more) with sophisticated control plane capabilities including deduplication, suppression, rerouting, and throttling.

The system is designed with pluggable infrastructure at every layer: swappable state backends (Redis, DynamoDB, Zookeeper), a polyglot rule engine supporting multiple DSLs (CEL, Rego, Drools-like, YAML), and an extensible provider architecture.

## **Key Features**

* Distributed action execution with configurable retry and backoff

* Global deduplication keyed by namespace, tenant, and action ID

* Rule-based suppression and rerouting with polyglot DSL support

* Pluggable state backends for distributed locking and coordination

* Extensible provider system for integrating new services

* Built on Tokio for high-throughput async execution

## **Design Philosophy**

Acteon follows a modular, crate-based architecture where each component can be used independently or composed together. The system prioritizes type safety, runtime flexibility, and operational observability.

# **Architecture Overview**

Acteon is organized as a workspace of interconnected crates, each responsible for a specific concern. The architecture follows a pipeline model where actions flow through intake, control plane evaluation, and execution phases.

## **High-Level Architecture**

┌─────────────────────────────────────────────────────────────────────┐  
│                            Acteon                                   │  
├─────────────────────────────────────────────────────────────────────┤  
│                                                                     │  
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────────────────┐ │  
│  │   Intake    │ →  │   Control   │ →  │       Executor          │ │  
│  │   (API)     │    │   Plane     │    │                         │ │  
│  └─────────────┘    └─────────────┘    └─────────────────────────┘ │  
│                            ↓                        ↓               │  
│         ┌──────────────────────────────┐    ┌─────────────┐        │  
│         │        Rule Engine           │    │  Providers  │        │  
│         │  ┌─────────┐ ┌───────────┐   │    └─────────────┘        │  
│         │  │ Dedup   │ │ Suppress  │   │                           │  
│         │  ├─────────┤ ├───────────┤   │                           │  
│         │  │ Reroute │ │ Throttle  │   │                           │  
│         │  └─────────┘ └───────────┘   │                           │  
│         └──────────────────────────────┘                           │  
│                            ↓                                        │  
│         ┌──────────────────────────────┐                           │  
│         │        State Store           │                           │  
│         │   (distributed locking)      │                           │  
│         └──────────────────────────────┘                           │  
│                                                                     │  
└─────────────────────────────────────────────────────────────────────┘

## **Crate Organization**

| Crate | Purpose |
| :---- | :---- |
| acteon-core | Shared types, traits, errors, action definitions |
| acteon-state | Distributed state abstraction and backend trait |
| acteon-state-redis | Redis/Valkey/DragonflyDB backend |
| acteon-state-dynamodb | AWS DynamoDB backend |
| acteon-state-zookeeper | Apache Zookeeper backend |
| acteon-state-etcd | etcd backend |
| acteon-state-postgres | PostgreSQL backend |
| acteon-rules | Rule engine core and IR |
| acteon-rules-cel | CEL DSL frontend |
| acteon-rules-rego | Rego/OPA DSL frontend |
| acteon-rules-drools | Drools-like DSL frontend |
| acteon-rules-yaml | YAML declarative frontend |
| acteon-rules-native | Rust proc-macro rules |
| acteon-provider | Provider abstraction and registry |
| acteon-executor | Async execution with retries |
| acteon-gateway | Main orchestration layer |
| acteon-server | Optional standalone HTTP/gRPC server |

# **Core Types**

The acteon-core crate defines the fundamental types used throughout the system. These types are designed for type safety, serialization, and clear semantics.

## **ActionKey**

The ActionKey is the composite key used for deduplication, suppression, and distributed locking operations. All control plane decisions are keyed by this structure.

\#\[derive(Debug, Clone, Hash, Eq, PartialEq)\]  
pub struct ActionKey {  
    pub namespace: Namespace,  
    pub tenant: Option\<TenantId\>,  
    pub action\_id: ActionId,  
    pub discriminator: Option\<String\>,  
}  
   
impl ActionKey {  
    pub fn lock\_key(\&self) \-\> String {  
        format\!("{}:{}:{}:{}",  
            self.namespace,  
            self.tenant.as\_deref().unwrap\_or("\_"),  
            self.action\_id,  
            self.discriminator.as\_deref().unwrap\_or("\_")  
        )  
    }  
}

## **ActionOutcome**

Every action dispatch results in an ActionOutcome that describes what happened to the action.

\#\[derive(Debug, Clone)\]  
pub enum ActionOutcome {  
    Executed { response: ProviderResponse, duration: Duration },  
    Deduplicated { original\_at: DateTime\<Utc\> },  
    Suppressed { rule: String, reason: String },  
    Rerouted { from: ProviderId, to: ProviderId },  
    Throttled { retry\_after: Duration },  
    Failed { error: ActionError, attempts: u32 },  
}

# **State Layer**

The state layer provides distributed state management with pluggable backends. It handles distributed locking, deduplication checks, counters, and action history tracking.

## **Design Goals**

* Clean abstraction that works across diverse backends

* Backend-appropriate key rendering for optimal performance

* Capability detection for advanced features (transactions, watches)

* Strong typing to prevent key format errors

## **StateStore Trait**

The core trait that all backends must implement:

\#\[async\_trait\]  
pub trait StateStore: Send \+ Sync \+ 'static {  
    /// Check if key exists, set if not. Returns true if newly set.  
    async fn check\_and\_set(  
        \&self,  
        key: \&StateKey,  
        ttl: Duration  
    ) \-\> Result\<bool, StateError\>;  
   
    /// Get a value by key  
    async fn get(\&self, key: \&StateKey) \-\> Result\<Option\<Vec\<u8\>\>, StateError\>;  
   
    /// Set a value with optional TTL  
    async fn set(  
        \&self,  
        key: \&StateKey,  
        value: Vec\<u8\>,  
        ttl: Option\<Duration\>  
    ) \-\> Result\<(), StateError\>;  
   
    /// Delete a key  
    async fn delete(\&self, key: \&StateKey) \-\> Result\<bool, StateError\>;  
   
    /// Increment counter, returns new value  
    async fn increment(  
        \&self,  
        key: \&StateKey,  
        ttl: Option\<Duration\>  
    ) \-\> Result\<u64, StateError\>;  
   
    /// Compare-and-swap for optimistic concurrency  
    async fn compare\_and\_swap(  
        \&self,  
        key: \&StateKey,  
        expected: Option\<&\[u8\]\>,  
        new\_value: Vec\<u8\>,  
        ttl: Option\<Duration\>,  
    ) \-\> Result\<CasResult, StateError\>;  
}

## **Distributed Locking**

The DistributedLock trait provides RAII-style locking with automatic release:

\#\[async\_trait\]  
pub trait DistributedLock: Send \+ Sync \+ 'static {  
    type Guard: LockGuard;  
   
    /// Acquire lock with TTL, blocks until acquired or timeout  
    async fn acquire(  
        \&self,  
        key: \&StateKey,  
        ttl: Duration,  
        timeout: Option\<Duration\>,  
    ) \-\> Result\<Self::Guard, StateError\>;  
   
    /// Try to acquire immediately  
    async fn try\_acquire(  
        \&self,  
        key: \&StateKey,  
        ttl: Duration,  
    ) \-\> Result\<Option\<Self::Guard\>, StateError\>;  
}  
   
\#\[async\_trait\]  
pub trait LockGuard: Send \+ Sync {  
    async fn extend(\&self, ttl: Duration) \-\> Result\<(), StateError\>;  
    async fn release(self) \-\> Result\<(), StateError\>;  
    fn is\_held(\&self) \-\> bool;  
}

## **StateKey**

Structured keys that render appropriately for each backend:

\#\[derive(Debug, Clone, Hash, Eq, PartialEq)\]  
pub struct StateKey {  
    pub namespace: String,  
    pub tenant: Option\<String\>,  
    pub kind: KeyKind,  
    pub id: String,  
}  
   
impl StateKey {  
    /// Redis-style: colon-separated  
    pub fn to\_redis\_key(\&self) \-\> String {  
        format\!("acteon:{}:{}:{}:{}",  
            self.namespace,  
            self.tenant.as\_deref().unwrap\_or("\_global"),  
            self.kind.as\_str(),  
            self.id  
        )  
    }  
   
    /// DynamoDB: partition \+ sort key  
    pub fn to\_dynamo\_keys(\&self) \-\> (String, String) {  
        let pk \= format\!("{}\#{}",  
            self.namespace,  
            self.tenant.as\_deref().unwrap\_or("\_global")  
        );  
        let sk \= format\!("{}\#{}", self.kind.as\_str(), self.id);  
        (pk, sk)  
    }  
   
    /// Zookeeper: path-based  
    pub fn to\_zk\_path(\&self) \-\> String {  
        format\!("/acteon/{}/{}/{}/{}",  
            self.namespace,  
            self.tenant.as\_deref().unwrap\_or("\_global"),  
            self.kind.as\_str(),  
            self.id  
        )  
    }  
}

## **Supported Backends**

| Backend | Consistency | Best For |
| :---- | :---- | :---- |
| Redis | Linearizable (single key) | Low latency, simple deployments |
| DynamoDB | Eventually consistent / Strong | AWS-native, serverless |
| Zookeeper | Strong (CP) | Coordination-heavy workloads |
| etcd | Strong (Raft) | Kubernetes environments |
| PostgreSQL | Strong (ACID) | Existing Postgres infrastructure |
| Memory | N/A | Testing and single-node development |

# **Rule Engine**

The rule engine is the brain of Acteon's control plane. It evaluates actions against a set of rules to determine whether they should be executed, deduplicated, suppressed, rerouted, or throttled.

## **Polyglot Design**

The rule engine supports multiple Domain-Specific Languages (DSLs) through a unified Internal Representation (IR). Each DSL has a frontend that parses source into IR, and a single execution engine evaluates the IR.

┌─────────────────────────────────────────────────────────────────┐  
│                     Rule Sources                                │  
├──────────┬──────────┬──────────┬──────────┬──────────┬─────────┤  
│   CEL    │   Rego   │  Drools  │   YAML   │  Native  │   ...   │  
└────┬─────┴────┬─────┴────┬─────┴────┬─────┴────┬─────┴────┬────┘  
     │          │          │          │          │          │  
     ▼          ▼          ▼          ▼          ▼          ▼  
┌─────────────────────────────────────────────────────────────────┐  
│                  Internal Representation (IR)                   │  
└─────────────────────────────────────────────────────────────────┘  
                              │  
                              ▼  
┌─────────────────────────────────────────────────────────────────┐  
│                     Execution Engine                            │  
└─────────────────────────────────────────────────────────────────┘

## **Supported DSLs**

| DSL | Style | Known From | Best For |
| :---- | :---- | :---- | :---- |
| CEL | Expression-based | Google, Kubernetes, Firebase | Simple predicates |
| Rego | Policy-as-code | Open Policy Agent | Complex policies |
| Drools-like | When/Then rules | JBoss, enterprise | Enterprise users |
| YAML | Declarative config | Simple configs | Non-programmers |
| Native Rust | Proc-macro | Type-safe code | Complex logic |

## **Expression IR**

All DSLs compile down to a common expression AST:

\#\[derive(Debug, Clone, PartialEq)\]  
pub enum Expr {  
    // Literals  
    Null,  
    Bool(bool),  
    Int(i64),  
    Float(f64),  
    String(String),  
    List(Vec\<Expr\>),  
    Map(Vec\<(Expr, Expr)\>),  
   
    // References  
    Ident(String),                     // action, state, env  
    Field(Box\<Expr\>, String),          // action.provider  
    Index(Box\<Expr\>, Box\<Expr\>),       // list\[0\]  
   
    // Operators  
    Unary(UnaryOp, Box\<Expr\>),  
    Binary(BinaryOp, Box\<Expr\>, Box\<Expr\>),  
   
    // Conditionals  
    Ternary(Box\<Expr\>, Box\<Expr\>, Box\<Expr\>),  
   
    // Function calls  
    Call(String, Vec\<Expr\>),  
   
    // Quantifiers (for Rego-style)  
    All(String, Box\<Expr\>, Box\<Expr\>),  
    Any(String, Box\<Expr\>, Box\<Expr\>),  
   
    // State access  
    StateGet(Box\<Expr\>),  
    StateCounter(Box\<Expr\>),  
    StateTimeSince(Box\<Expr\>),  
}

## **Rule Actions**

Rules can produce the following verdicts:

pub enum RuleAction {  
    Allow,  
    Deny { reason: Expr },  
    Deduplicate { key: Expr, window: Expr },  
    Suppress { reason: Expr, duration: Option\<Expr\> },  
    Reroute { to: Expr },  
    Throttle { rate: Expr, window: Expr },  
    Modify { mutations: Vec\<Mutation\> },  
    Custom { handler: String, params: Vec\<(String, Expr)\> },  
}

# **DSL Examples**

This section provides concrete examples of rules written in each supported DSL.

## **CEL Example**

CEL (Common Expression Language) is ideal for simple, expression-based rules:

// rules/slack\_dedup.cel  
   
@rule(name \= "slack-dedup", priority \= 100\)  
@on(deduplicate, key \= action.dedup\_key(), window \= duration("5m"))  
   
action.provider \== "slack"  
    && action.payload.channel.startsWith("\#alerts")  
    && state.seen(action.dedup\_key())  
   
   
@rule(name \= "reroute-to-backup", priority \= 50\)  
@on(reroute, to \= "slack-backup")  
   
action.provider \== "slack"  
    && env.circuit\_breaker("slack-primary").is\_open()

## **Rego Example**

Rego excels at complex policy logic with data-driven decisions:

\# rules/alert\_policies.rego  
   
package acteon.rules  
   
import future.keywords.if  
import future.keywords.in  
   
\# Deduplicate identical alerts within 5 minutes  
deduplicate\_alert if {  
    input.action.provider \== "pagerduty"  
    input.action.type \== "trigger"  
      
    key := sprintf("%s:%s", \[  
        input.action.payload.service,  
        input.action.payload.alert\_key  
    \])  
    state.seen(key, "5m")  
}  
   
\# Suppress alerts during maintenance windows  
suppress\_maintenance if {  
    input.action.provider in \["pagerduty", "slack"\]  
    maintenance := data.maintenance\_windows\[\_\]  
    time.now\_ns() \>= maintenance.start  
    time.now\_ns() \<= maintenance.end  
    input.action.payload.service in maintenance.services  
}

## **YAML Example**

YAML provides a declarative approach for simple rules without learning a DSL:

\# rules/simple.yaml  
   
rules:  
  \- name: deduplicate-slack-alerts  
    priority: 100  
    description: Prevent duplicate Slack messages  
      
    when:  
      all:  
        \- field: action.provider  
          equals: slack  
        \- field: action.payload.channel  
          starts\_with: "\#alerts"  
        \- state.seen:  
            key: "{{ action.dedup\_key }}"  
            window: 5m  
      
    then:  
      deduplicate:  
        key: "{{ action.dedup\_key }}"  
        window: 5m  
   
  \- name: throttle-pagerduty  
    priority: 90  
      
    when:  
      all:  
        \- field: action.provider  
          equals: pagerduty  
        \- state.counter:  
            key: "{{ action.payload.service }}:alerts"  
            window: 1m  
            greater\_than: 10  
      
    then:  
      throttle:  
        rate: 10  
        window: 1m

## **Native Rust Example**

For complex logic that's difficult to express in DSLs, use native Rust with proc macros:

use acteon\_rules\_native::rule;  
   
\#\[rule(name \= "smart-reroute", priority \= 50)\]  
async fn smart\_reroute(ctx: \&RuleContext) \-\> RuleResult {  
    // Complex logic that's hard in DSLs  
    let health \= ctx.env  
        .health\_check(\&ctx.action.provider)  
        .await?;  
      
    if health.latency\_p99 \> Duration::from\_secs(5) {  
        let backup \= find\_best\_backup(  
            \&ctx.action.provider,  
            \&ctx.env  
        ).await?;  
        return RuleResult::Reroute { to: backup };  
    }  
      
    RuleResult::Allow  
}

# **Provider System**

Providers are the integrations that execute actions against external services. The provider system is designed to be extensible, allowing new providers to be added without modifying core code.

## **Provider Trait**

\#\[async\_trait\]  
pub trait Provider: Send \+ Sync {  
    type Action: Send;  
    type Response: Send;  
    type Error: std::error::Error \+ Send \+ Sync;  
   
    fn name(\&self) \-\> &'static str;  
      
    async fn execute(  
        \&self,  
        action: Self::Action  
    ) \-\> Result\<Self::Response, Self::Error\>;  
      
    async fn health\_check(\&self) \-\> Result\<(), Self::Error\>;  
}

## **Supported Providers**

| Provider | Description | Status |
| :---- | :---- | :---- |
| Slack | Messages, reactions, channel management | Planned |
| PagerDuty | Incidents, alerts, escalations | Planned |
| Twilio | SMS, voice, WhatsApp | Planned |
| AWS SQS | Queue messaging | Planned |
| AWS SNS | Pub/sub notifications | Planned |
| LLM Gateway | OpenAI, Anthropic, local models | Planned |
| Webhook | Generic HTTP callbacks | Planned |
| Email (SMTP) | Transactional email | Planned |

## **Provider Registry**

The ProviderRegistry manages provider instances and provides lookup by name:

pub struct ProviderRegistry {  
    providers: HashMap\<String, Arc\<dyn DynProvider\>\>,  
}  
   
impl ProviderRegistry {  
    pub fn register\<P: Provider \+ 'static\>(  
        \&mut self,  
        provider: P  
    ) {  
        self.providers.insert(  
            provider.name().to\_string(),  
            Arc::new(provider)  
        );  
    }  
      
    pub fn get(\&self, name: \&str) \-\> Option\<Arc\<dyn DynProvider\>\> {  
        self.providers.get(name).cloned()  
    }  
}

# **Gateway Orchestration**

The Gateway is the main entry point that ties all components together. It orchestrates the flow from action intake through rule evaluation to execution.

## **Pipeline Flow**

impl Gateway {  
    pub async fn dispatch(  
        \&self,  
        action: Action  
    ) \-\> Result\<ActionOutcome\> {  
        let key \= action.key();  
          
        // 1\. Acquire distributed lock for this action key  
        let \_lock \= self.state  
            .acquire\_lock(\&key.lock\_key(), Duration::from\_secs(30))  
            .await?;  
          
        // 2\. Run through rule engine  
        let verdict \= self.rules  
            .evaluate(\&action, \&key, &\*self.state)  
            .await?;  
          
        // 3\. Act on verdict  
        let outcome \= match verdict {  
            RuleVerdict::Allow \=\> {  
                let provider \= self.providers.get(\&action.provider)?;  
                self.executor.execute(provider, action).await  
            }  
            RuleVerdict::Deduplicate \=\> {  
                ActionOutcome::Deduplicated {  
                    original\_at: Utc::now()  
                }  
            }  
            RuleVerdict::Suppress { reason } \=\> {  
                ActionOutcome::Suppressed {  
                    rule: "suppress".into(),  
                    reason  
                }  
            }  
            RuleVerdict::Reroute { to } \=\> {  
                let provider \= self.providers.get(\&to)?;  
                self.executor.execute(provider, action).await  
            }  
            RuleVerdict::Throttle { retry\_after } \=\> {  
                ActionOutcome::Throttled { retry\_after }  
            }  
        };  
          
        // 4\. Record outcome for observability  
        self.state.record\_outcome(\&key, \&outcome).await?;  
          
        Ok(outcome)  
    }  
}

## **Builder Pattern**

The Gateway is constructed using a builder pattern for flexible configuration:

let state \= StateBuilder::redis("redis://localhost:6379")  
    .build()  
    .await?;  
   
let mut engine \= RuleEngine::new();  
engine.load\_directory(Path::new("./rules"))?;  
   
let gateway \= Acteon::builder()  
    .state(state)  
    .rules(engine)  
    .provider(Slack::from\_env())  
    .provider(PagerDuty::from\_env())  
    .provider(Twilio::from\_env())  
    .build()  
    .await?;  
   
// Dispatch an action  
let outcome \= gateway.dispatch(  
    Action::slack("ops-alerts")  
        .message("Deployment complete")  
).await?;

# **Project Structure**

The complete project structure organized as a Cargo workspace:

acteon/  
├── Cargo.toml                    \# Workspace manifest  
├── README.md  
├── LICENSE-MIT  
├── LICENSE-APACHE  
│  
├── acteon-core/                  \# Shared types and traits  
│   ├── Cargo.toml  
│   └── src/  
│       ├── lib.rs  
│       ├── action.rs             \# Action type definitions  
│       ├── key.rs                \# ActionKey composite key  
│       ├── outcome.rs            \# ActionOutcome enum  
│       └── context.rs            \# ActionContext metadata  
│  
├── acteon-state/                 \# State abstraction  
│   ├── Cargo.toml  
│   └── src/  
│       ├── lib.rs  
│       ├── traits/  
│       │   ├── mod.rs  
│       │   ├── store.rs          \# StateStore trait  
│       │   ├── lock.rs           \# DistributedLock trait  
│       │   └── atomic.rs         \# AtomicOperations trait  
│       ├── key.rs                \# StateKey utilities  
│       ├── error.rs  
│       └── testing.rs            \# MockStateStore  
│  
├── acteon-state-memory/          \# In-memory backend  
├── acteon-state-redis/           \# Redis backend  
├── acteon-state-dynamodb/        \# DynamoDB backend  
├── acteon-state-zookeeper/       \# Zookeeper backend  
├── acteon-state-etcd/            \# etcd backend  
├── acteon-state-postgres/        \# PostgreSQL backend  
│  
├── acteon-rules/                 \# Rule engine core  
│   ├── Cargo.toml  
│   └── src/  
│       ├── lib.rs  
│       ├── ir/                   \# Internal Representation  
│       │   ├── mod.rs  
│       │   ├── expr.rs           \# Expression AST  
│       │   ├── rule.rs           \# Rule IR  
│       │   ├── action.rs         \# Rule actions  
│       │   └── optimize.rs       \# IR optimization  
│       ├── engine/  
│       │   ├── mod.rs  
│       │   ├── executor.rs       \# IR execution  
│       │   ├── context.rs        \# EvalContext  
│       │   └── builtins.rs       \# Built-in functions  
│       ├── traits.rs  
│       └── registry.rs  
│  
├── acteon-rules-cel/             \# CEL frontend  
├── acteon-rules-rego/            \# Rego frontend  
├── acteon-rules-drools/          \# Drools-like frontend  
├── acteon-rules-yaml/            \# YAML frontend  
├── acteon-rules-native/          \# Rust proc-macro  
│  
├── acteon-provider/              \# Provider abstraction  
│   ├── Cargo.toml  
│   └── src/  
│       ├── lib.rs  
│       ├── trait.rs              \# Provider trait  
│       ├── registry.rs           \# ProviderRegistry  
│       └── health.rs  
│  
├── acteon-executor/              \# Execution engine  
│   ├── Cargo.toml  
│   └── src/  
│       ├── lib.rs  
│       ├── executor.rs  
│       ├── retry.rs              \# Backoff strategies  
│       ├── dlq.rs                \# Dead letter queue  
│       └── batch.rs  
│  
├── acteon-gateway/               \# Main orchestration  
│   ├── Cargo.toml  
│   └── src/  
│       ├── lib.rs  
│       ├── gateway.rs  
│       ├── intake.rs  
│       └── pipeline.rs  
│  
├── acteon-server/                \# Standalone server  
│   ├── Cargo.toml  
│   └── src/  
│       ├── main.rs  
│       ├── api.rs                \# HTTP/gRPC  
│       └── config.rs  
│  
├── providers/                    \# Provider implementations  
│   ├── acteon-slack/  
│   ├── acteon-pagerduty/  
│   ├── acteon-twilio/  
│   ├── acteon-sqs/  
│   ├── acteon-sns/  
│   ├── acteon-llm/  
│   └── acteon-webhook/  
│  
├── rules/                        \# Example rules  
│   ├── cel/  
│   ├── rego/  
│   ├── yaml/  
│   └── drools/  
│  
└── examples/  
    ├── basic.rs  
    └── multi\_provider.rs

# **Configuration**

Acteon uses a hierarchical configuration system supporting environment variables, config files, and programmatic configuration.

## **Example Configuration**

\# acteon.toml  
   
\[state\]  
backend \= "redis"  
url \= "redis://localhost:6379"  
prefix \= "acteon"  
   
\[state.pool\]  
min\_connections \= 5  
max\_connections \= 20  
   
\[rules\]  
directory \= "./rules"  
watch \= true  \# Hot reload on changes  
   
\[providers.slack\]  
enabled \= true  
token \= "${SLACK\_TOKEN}"  
default\_channel \= "\#alerts"  
   
\[providers.pagerduty\]  
enabled \= true  
api\_key \= "${PAGERDUTY\_API\_KEY}"  
default\_service \= "infrastructure"  
   
\[executor\]  
max\_retries \= 3  
base\_backoff\_ms \= 100  
max\_backoff\_ms \= 30000  
   
\[server\]  
host \= "0.0.0.0"  
port \= 8080  
grpc\_port \= 9090  
   
\[observability\]  
metrics\_enabled \= true  
tracing\_enabled \= true  
log\_level \= "info"

# **API Reference**

This section provides a quick reference for the main APIs.

## **Gateway API**

| Method | Description |
| :---- | :---- |
| dispatch(action) | Execute an action through the pipeline |
| dispatch\_batch(actions) | Execute multiple actions in parallel |
| health\_check() | Check gateway and provider health |
| metrics() | Get current metrics snapshot |
| reload\_rules() | Hot reload rules from configured directory |

## **StateStore API**

| Method | Description |
| :---- | :---- |
| check\_and\_set(key, ttl) | Atomic check-and-set for deduplication |
| get(key) | Retrieve value by key |
| set(key, value, ttl) | Store value with optional TTL |
| delete(key) | Remove a key |
| increment(key, ttl) | Atomic counter increment |
| compare\_and\_swap(key, expected, new, ttl) | CAS operation |
| acquire\_lock(key, ttl, timeout) | Acquire distributed lock |
| try\_acquire\_lock(key, ttl) | Non-blocking lock attempt |

## **RuleEngine API**

| Method | Description |
| :---- | :---- |
| evaluate(action, state, env) | Evaluate action against all rules |
| load\_file(path) | Load rules from a single file |
| load\_directory(path) | Load all rules from directory |
| register\_frontend(frontend) | Add support for a new DSL |
| list\_rules() | Get all loaded rules |
| enable\_rule(id) | Enable a specific rule |
| disable\_rule(id) | Disable a specific rule |

# **Future Considerations**

Areas for future development and enhancement.

## **Planned Features**

* FoundationDB state backend for globally distributed deployments

* WebAssembly (WASM) rule plugins for sandboxed custom logic

* GraphQL API alongside REST and gRPC

* Distributed tracing integration with OpenTelemetry

* Rule versioning and gradual rollout capabilities

* Multi-region deployment patterns and documentation

* Admin UI for rule management and monitoring

## **Performance Targets**

| Metric | Target |
| :---- | :---- |
| Throughput | \> 100,000 actions/second per node |
| Latency (p50) | \< 1ms for rule evaluation |
| Latency (p99) | \< 10ms end-to-end (excluding provider) |
| State operations | \< 5ms for Redis/DynamoDB |
| Memory footprint | \< 100MB base \+ rules |

## **On the Name**

The name Acteon draws from the Greek myth of Actaeon, a hunter of extraordinary skill who was transformed by the goddess Artemis into a stag -- the very thing he pursued. In much the same way, Acteon the system stands at the boundary between intent and execution: actions enter as raw requests and are transformed -- deduplicated, rerouted, throttled, or dispatched -- before they ever reach the outside world. The stag in the logo is a nod to that transformation, a reminder that what you send in is not always what comes out the other side.

— **Acteon**: Actions Forged in Rust —