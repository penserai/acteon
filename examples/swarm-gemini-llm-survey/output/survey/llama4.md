# Llama 4 (Meta) Research - March 2026

## Overview
Llama 4 is Meta's latest frontier-class open-weight model series, released in 2025 and 2026. It marks a significant shift to a Mixture-of-Experts (MoE) architecture and introduces industry-leading context windows.

## Model Architecture & Sizes
Llama 4 uses a **Mixture-of-Experts (MoE)** design, activating only a subset of parameters per token to optimize inference speed and efficiency.

| Model | Total Parameters | Active Parameters | Architecture | Primary Use Case |
| :--- | :--- | :--- | :--- | :--- |
| **Llama 4 Scout** | 109B | 17B | MoE (16 experts) | Edge/Single-GPU, Long-context |
| **Llama 4 Maverick** | 400B | 17B | MoE (128 experts) | Flagship, Multimodal, Coding |
| **Llama 4 Behemoth** | ~2T | 288B | MoE (16 experts) | Frontier Research (Teacher model) |

## Benchmarks
Llama 4 Maverick competes with GPT-5 and Claude 4 class models.

| Benchmark | Score (Maverick) | Notes |
| :--- | :--- | :--- |
| **MMLU** | ~85.5% | Strong general reasoning, slightly behind closed-source leaders. |
| **HumanEval** | ~77.6% | Optimized for software engineering and long-context codebases. |
| **MATH** | ~61.2% | Significant improvement over Llama 3 in formal reasoning. |
| **GPQA** | ~67.1% | High-level expert reasoning. |
| **AIME 2025** | ~39% | Advanced competitive math capabilities. |

## Context Length
Llama 4 introduced "ultra-long" context windows, surpassing most contemporary models.
- **Llama 4 Scout:** Up to **10 million tokens**.
- **Llama 4 Maverick:** **1 million tokens**.
- **Llama 4 Behemoth:** **1 million+ tokens**.

## License & Availability
- **License:** Llama 4 Community License Agreement.
- **Open-Weight:** Weights are available via Hugging Face and Meta's official portal.
- **Commercial Terms:** Free for most users; requires a specific license for companies exceeding **700 million monthly active users**.
- **Usage Restrictions:** Prohibits the use of model outputs to train competing LLMs.

## Key Features
- **Native Multimodality:** Supports simultaneous text and image processing.
- **Efficiency:** Scout is optimized for single-GPU (H100/B200) execution when quantized.
- **Reasoning:** Enhanced Chain-of-Thought (CoT) and formal reasoning capabilities compared to previous generations.
