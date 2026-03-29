# Early 2026 Open-Source LLM Survey

This directory contains research reports for the leading open-weight and open-source Large Language Models (LLMs) as of March 2026. The survey covers a diverse range of architectures, from dense small language models to massive Mixture-of-Experts (MoE) systems.

## Model Comparison Summary

The following table compares the flagship or notable variants from each model family.

| Model | Primary Sizes | Context Length | MMLU | HumanEval / Coding | MATH / Reasoning | License | Unique Strengths |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **DeepSeek V3/R1** | 671B (MoE), 1.5B-70B (Distilled) | 128k | 88.3% (GPQA)* | 73.1% (SWE-bench) | 97.3% (MATH-500) | MIT | SOTA math/reasoning, extreme MoE efficiency, visible CoT. |
| **Mistral Large 3** | 675B (MoE), 24B (Small), 3B-14B (Mini) | 256k | 85.5% | 92.0% | 68.2% (AIME) | Apache 2.0 | Granular MoE (1:16.5 sparsity), high-performance coding. |
| **Llama 4 Maverick** | 109B, 400B (MoE), ~2T | 1M - 10M | ~85.5% | ~77.6% | ~61.2% | Llama 4 Community | Massive context windows (up to 10M), frontier performance. |
| **Qwen 3** | 0.6B to 235B (Dense/MoE) | 128k | - | 47.2 (LCB) | Top-tier (AIME25) | Apache 2.0 | Hybrid reasoning (Thinking mode), 119 languages. |
| **Phi-4** | 3.8B (mini), 14.7B (Base) | 128k | 67.3% (mini) | - | 64.0% (mini) | MIT | SOTA performance-per-parameter, on-device reasoning. |
| **Command R+** | 35B, 104B | 128k | 75.7% | - | 70.7% (GSM8k) | CC-BY-NC 4.0 | RAG-optimized, inline citations, agentic tool-use. |
| **Gemma 3** | 270M to 27B | 128k | 67.5 (Pro) | - | - | Gemma Terms | Native multimodality (image/text), safety ecosystem. |

*\*Note: DeepSeek metrics are reported for R1/V3.2. Benchmarks vary by model report and may include proxies like GPQA or AIME where standard MMLU/HumanEval was not the primary focus.*

## Dominant Trends in Early 2026

The open-source LLM landscape in early 2026 is defined by several key technological shifts:

1.  **The MoE Standard**: The Mixture-of-Experts (MoE) architecture has become the default for frontier-class open models. Innovations like Mistral's "Granular MoE" and DeepSeek's "Multi-head Latent Attention" have significantly reduced the compute requirements for hosting massive knowledge bases.
2.  **Native Reasoning (Thinking Mode)**: Led by DeepSeek R1 and followed by Qwen 3 and Phi-4 Reasoning, models now increasingly incorporate internal "Thinking" steps or Chain-of-Thought (CoT) tokens. This allows models to "pause and reflect" on complex problems, dramatically improving performance in math, science, and coding.
3.  **Context in the Millions**: Context windows have expanded from the 32k/128k standard of 2024 to 1M+ tokens in 2026. Llama 4 Scout's 10-million-token window represents the current frontier, enabling the processing of entire repositories or libraries in a single prompt.
4.  **Permissive Licensing for Adoption**: There is a clear trend toward more permissive licensing. DeepSeek and Phi-4 utilize the **MIT License**, while Mistral and Qwen have largely adopted **Apache 2.0**. This has accelerated commercial integration and community fine-tuning.
5.  **Ubiquitous Multimodality**: Native multimodal support (text, image, and sometimes audio/video) is no longer reserved for flagship models. Smaller models like Gemma 3 (4B) and Phi-4 (5.6B) now offer sophisticated vision capabilities out of the box.
6.  **Data Efficiency over Scale**: Microsoft’s Phi family and Google’s Gemma series continue to prove that high-quality synthetic data and curriculum-based training can produce small models (3B-12B) that rival the reasoning capabilities of previous generation 70B+ models.
