# DeepSeek R1 & V3 Model Survey (March 2026)

## 1. Overview
DeepSeek has established itself as a leader in high-efficiency, high-performance LLMs. As of March 2026, the primary models are the **V3 series** (general-purpose) and the **R1 series** (reasoning-focused).

## 2. Architecture Details
Both V3 and R1 leverage a sophisticated **Mixture-of-Experts (MoE)** architecture designed for extreme computational efficiency.

- **Parameters:** ~671B to 685B total parameters.
- **Active Parameters:** Only **~37B activated per token**, allowing for faster inference than dense models of similar total size.
- **Multi-head Latent Attention (MLA):** A proprietary mechanism that drastically reduces KV cache size (by up to 90%), enabling 128K context windows with minimal memory overhead.
- **DeepSeek Sparse Attention (DSA):** Introduced in later versions (V3.2) to optimize long-context processing by reducing complexity from $O(N^2)$ to a scalable sparse format.
- **Training Strategy:**
    - Pre-trained on 14.8T+ tokens.
    - **Group Relative Policy Optimization (GRPO):** A reinforcement learning (RL) algorithm that enables reasoning capabilities without requiring massive amounts of Supervised Fine-Tuning (SFT) data.

## 3. Reasoning Capabilities (Chain of Thought)
DeepSeek-R1 is specifically optimized for complex reasoning via "Thinking Mode."

- **Visible Chain-of-Thought (CoT):** The model generates internal "thinking" tokens (wrapped in `<think>` tags) where it performs step-by-step deduction, self-correction, and verification.
- **Inference Scaling:** DeepSeek demonstrates that reasoning performance scales with the number of thinking tokens used.
- **Self-Evolution:** Through pure RL (DeepSeek-R1-Zero), the models autonomously discovered behaviors like reflection and multi-path verification.
- **System Prompting:** Users can trigger or suppress the thinking process via specific system instructions.

### Example Thinking Output:
```text
<think>
1. The user wants to find the derivative of x^x.
2. Use logarithmic differentiation. Let y = x^x.
3. ln(y) = x ln(x).
4. Differentiate both sides: (1/y) y' = ln(x) + x(1/x) = ln(x) + 1.
5. Multiply by y: y' = x^x (ln(x) + 1).
6. Result: x^x (1 + ln(x)).
</think>
The derivative of $x^x$ is $x^x (1 + \ln x)$.
```

## 4. Benchmarks
DeepSeek models consistently outperform or match proprietary leaders (GPT-5, Gemini 3.0 Pro, o1/o3).

| Benchmark | DeepSeek-R1 | DeepSeek-V3.2 | Notes |
| :--- | :--- | :--- | :--- |
| **AIME 2025** | 91.4% (Pass@1) | Gold Medal | Top-tier math reasoning |
| **MATH-500** | 97.3% | 99.2% | Near-perfect math score |
| **Codeforces** | 1930 (Rating) | 2708 (Rating) | Competitive programming |
| **SWE-bench** | 57.6% | 73.1% | Real-world software engineering |
| **GPQA Diamond** | 81.0% | 88.3% | Graduate-level hard science |

## 5. Licensing & Open Source
DeepSeek maintains a highly permissive stance compared to its competitors.

- **License:** **MIT License**. This allows for free use, modification, and commercial distribution.
- **Weights:** Model weights are publicly available on Hugging Face.
- **Distillation:** DeepSeek provides "distilled" versions (1.5B to 70B) based on Llama/Qwen architectures, bringing R1-level reasoning to smaller footprints.

## 6. Implementation Examples

### Using DeepSeek API (OpenAI-compatible)
```python
import openai

client = openai.OpenAI(
    api_key="YOUR_DEEPSEEK_API_KEY",
    base_url="https://api.deepseek.com/v1"
)

response = client.chat.completions.create(
    model="deepseek-reasoner", # or deepseek-chat for V3
    messages=[
        {"role": "system", "content": "You are a helpful assistant."},
        {"role": "user", "content": "Prove that there are infinitely many primes."}
    ],
    stream=False
)

print(response.choices[0].message.content)
```

### Loading Distilled Model via Transformers
```python
from transformers import AutoTokenizer, AutoModelForCausalLM

model_id = "deepseek-ai/DeepSeek-R1-Distill-Llama-70B"
tokenizer = AutoTokenizer.from_pretrained(model_id)
model = AutoModelForCausalLM.from_pretrained(model_id, device_map="auto")

inputs = tokenizer("Explain the Fermat's Last Theorem.", return_tensors="pt").to("cuda")
outputs = model.generate(**inputs, max_new_tokens=512)
print(tokenizer.decode(outputs[0]))
```
