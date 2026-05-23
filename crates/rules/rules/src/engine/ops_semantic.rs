use crate::engine::context::{EvalContext, SemanticMatchDetail};
use crate::engine::eval::eval;
use crate::engine::value::Value;
use crate::error::RuleError;
use crate::ir::expr::Expr;

/// Evaluate a semantic match condition.
///
/// Uses the embedding support in the context to compute cosine similarity
/// between the text and the topic. Returns `true` when the similarity meets
/// or exceeds the threshold.
pub(crate) async fn eval_semantic_match(
    topic: &str,
    threshold: f64,
    text_field: Option<&Expr>,
    ctx: &EvalContext<'_>,
) -> Result<Value, RuleError> {
    let embedding = ctx.embedding.as_ref().ok_or_else(|| {
        RuleError::Evaluation("semantic_match requires embedding support".to_owned())
    })?;

    let text = if let Some(expr) = text_field {
        let val = Box::pin(eval(expr, ctx)).await?;
        val.display_string()
    } else {
        ctx.action.payload.to_string()
    };

    if text.is_empty() || text == "null" {
        return Ok(Value::Bool(false));
    }

    let similarity = embedding.similarity(&text, topic).await?;

    // Record the detail for the playground trace.
    if let Some(ref tracker) = ctx.access_tracker {
        tracker.set_semantic_detail(SemanticMatchDetail {
            extracted_text: text,
            topic: topic.to_owned(),
            similarity,
            threshold,
        });
    }

    Ok(Value::Bool(similarity >= threshold))
}
