# Concepts

This section explains the core ideas behind Acteon. Understanding these concepts is essential for using the system effectively.

<div class="grid" markdown>

<div class="card" markdown>

### [Architecture](architecture.md)

How Acteon is structured as a workspace of Rust crates, with each component handling a specific concern.

</div>

<div class="card" markdown>

### [Actions & Outcomes](actions.md)

The `Action` type that flows through the system and the `ActionOutcome` variants that describe what happened to it.

</div>

<div class="card" markdown>

### [The Dispatch Pipeline](pipeline.md)

Step-by-step walkthrough of how an action is processed — from intake through rule evaluation, execution, and audit recording.

</div>

<div class="card" markdown>

### [Rule System](rules.md)

How rules are defined, evaluated, and matched against actions to determine what happens.

</div>

<div class="card" markdown>

### [Providers](providers.md)

The provider abstraction that allows Acteon to dispatch actions to any external service.

</div>

</div>
