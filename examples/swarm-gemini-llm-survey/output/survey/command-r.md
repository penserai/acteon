# Cohere Command R+ Research Report

## 1. Overview
Command R+ is Cohere's state-of-the-art 104-billion parameter model, released in April 2024. It is part of the Command R series (which also includes the 35B Command R model), specifically optimized for enterprise-grade workloads, complex Retrieval-Augmented Generation (RAG), and multi-step tool use (agents).

## 2. Architecture
- **Model Size:** 104 Billion parameters.
- **Type:** Optimized transformer architecture.
- **Context Window:** 128,000 tokens.
- **Tokenizer:** Highly efficient for non-English text, achieving significant token count reductions (up to 57%) for key business languages compared to other models.
- **Multilingual Support:** Optimized for 10 languages: English, French, Spanish, Italian, German, Brazilian Portuguese, Japanese, Korean, Arabic, and Simplified Chinese.
- **Training:** Post-pretraining includes Supervised Fine-Tuning (SFT) and preference training (RLHF/DPO) to align with human preferences for helpfulness and safety.

## 3. RAG & Tool-Use Capabilities
Command R+ is specifically "RAG-optimized," meaning it is trained to handle long-context retrieval tasks with high precision.

### Key RAG Features
- **Inline Citations:** The model generates responses with specific citations to the provided document snippets, reducing hallucinations and enabling verification.
- **Accuracy Modes:**
  - **Accurate Mode:** First predicts relevant documents, then cited documents, then generates the answer.
  - **Fast Mode:** Directly generates the answer with citations to reduce latency.
- **Reduced Hallucinations:** In internal benchmarks, Command R+ demonstrates accuracy levels competitive with GPT-4 Turbo while providing better citation quality.

### Agentic & Tool-Use Features
- **Multi-Step Tool Use:** Capable of reasoning through an **Action → Observation → Reflection** loop, allowing it to combine multiple tools over several steps.
- **Self-Correction:** It can identify and fix its own errors during tool execution.
- **Single-Step (Function Calling):** Efficiently decides which tools to call and with what parameters in a single turn.

## 4. Benchmarks
Command R+ performs competitively with top-tier closed-weights models on standard LLM and specialized benchmarks.

| Metric | Command R+ Score | Context / Comparison |
| :--- | :--- | :--- |
| **MMLU** | 75.7% | Competitive with DBRX and GPT-4 class models. |
| **GSM8k** | 70.7% | Strong mathematical reasoning. |
| **ARC (Challenge)** | 70.99% | Robust factual reasoning. |
| **Microsoft ToolTalk** | State-of-the-art | Comparable to GPT-4 Turbo and Claude 3 Sonnet. |
| **Multilingual** | High | Significantly outperforms many models in non-English RAG. |

## 5. Licensing & Availability
- **License:** **CC-BY-NC 4.0** (Creative Commons Attribution-NonCommercial 4.0 International).
- **Usage:** "Open weights" release. Free for research and non-commercial use. Commercial use typically requires a separate agreement with Cohere or access through cloud partners (Azure, AWS, OCI).
- **Compliance:** Must adhere to Cohere’s Acceptable Use Policy.

## 6. API Implementation Example (Python)
The following example shows how to use the Cohere V2 API to perform a RAG-based tool-use interaction.

```python
import cohere
import json

co = cohere.ClientV2(api_key="YOUR_API_KEY")

# Define search tool
tools = [{
    "type": "function",
    "function": {
        "name": "search_docs",
        "description": "Searches documentation for company policies.",
        "parameters": {
            "type": "object",
            "properties": {
                "query": {"type": "string"}
            },
            "required": ["query"]
        }
    }
}]

# Initial prompt
messages = [{"role": "user", "content": "What is the vacation policy?"}]

# Model decides to call the tool
response = co.chat(
    model="command-r-plus",
    messages=messages,
    tools=tools
)

if response.message.tool_calls:
    # Handle tool calls (execute search locally)
    # ... logic to run search_docs and get results ...
    results = [{"title": "Vacation Policy", "text": "20 days PTO per year."}]
    
    # Add results back to context
    messages.append(response.message)
    messages.append({
        "role": "tool",
        "tool_call_id": response.message.tool_calls[0].id,
        "content": json.dumps(results)
    })

    # Final grounded generation
    final_response = co.chat(
        model="command-r-plus",
        messages=messages,
        tools=tools
    )
    print(final_response.message.content[0].text)
    # Access citations via final_response.message.citations
```

## 7. Comparison: Command R vs. Command R+
| Feature | Command R | Command R+ |
| :--- | :--- | :--- |
| Parameters | 35B | 104B |
| Best Use Case | Efficiency, high throughput | Complex reasoning, agentic tasks |
| Context Window | 128k | 128k |
| RAG Optimization | Native | Advanced |
