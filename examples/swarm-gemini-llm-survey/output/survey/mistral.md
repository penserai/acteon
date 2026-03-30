# Mistral AI: Model Survey (March 2026)

This document provides a comprehensive overview of Mistral AI's model portfolio, focusing on the latest releases (Mistral 3 generation), architectural innovations, benchmarks, and licensing terms.

## 1. Latest Model Releases (2025-2026)

As of March 2026, Mistral AI has transitioned into its "Mistral 3" era, characterized by massive Mixture-of-Experts (MoE) architectures and a shift toward unified multimodal capabilities.

### **The Mistral 3 Family**
*   **Mistral Large 3 (Dec 2025):** The current flagship flagship model. A massive "Granular MoE" system with **675B total parameters** (41B active). It is natively multimodal (text/image) and supports a **256k context window**.
*   **Ministral 3 (Dec 2025):** A series of edge-optimized dense models (3B, 8B, and 14B) designed for local, low-latency deployment on consumer hardware.
*   **Mistral Small 3.1 (Jan 2026):** A 24B parameter dense model optimized for cost-efficiency while maintaining high reasoning capabilities.

### **Specialized & Reasoning Models**
*   **Magistral 1.2 (Feb 2026):** Mistral's premier **reasoning models** (competition for OpenAI’s o-series). 
    *   **Magistral Small (24B):** Open-weight reasoning model.
    *   **Magistral Medium:** Proprietary, high-performance reasoning engine.
*   **Devstral 2 (Feb 2026):** A specialized coding agent model designed for complex multi-file software engineering tasks.
*   **Voxtral TTS (March 2026):** A 4B parameter text-to-speech model released with open weights, supporting 9 languages and ultra-low latency voice cloning.

---

## 2. Architecture: The Evolution of MoE

Mistral AI is a pioneer in the **Mixture-of-Experts (MoE)** architecture, which allows for large model capacity without the proportional increase in inference cost.

### **Standard MoE (Mixtral 8x7B / 8x22B)**
*   **Total Experts:** 8.
*   **Routing:** Top-2 gating mechanism. For every token, the model selects the two most relevant experts.
*   **Efficiency:** ~1:3.6 active-to-total parameter ratio.
*   **Performance:** Provided GPT-4 class performance (8x22B) with significantly lower hardware requirements than dense models of similar quality.

### **Granular MoE (Mistral Large 3)**
*   **Concept:** Instead of a few large experts, the model uses **thousands of granular expert subnetworks**.
*   **Routing Logic:** A complex gating network activates a specific subset of these "fine-grained" experts to reach the **41B active parameter** threshold.
*   **Sparsity Ratio:** ~1:16.5 (675B total / 41B active). This extreme sparsity allows for a much larger knowledge base (675B) to be accessed with the compute cost of a mid-sized model (41B).
*   **Hardware Integration:** Co-designed with NVIDIA to utilize **NVFP4 quantization** and optimized Blackwell kernels, enabling the full 675B model to run on a single 8-GPU node.

---

## 3. Benchmarks & Performance

| Model | MMLU | HumanEval (Coding) | AIME 2024 (Math/Reasoning) | Context Window |
| :--- | :--- | :--- | :--- | :--- |
| **Mistral Large 3** | 85.5% | 92.0% | 68.2% | 256k |
| **Magistral Med 1.2**| 84.1% | 88.5% | **73.6%** | 128k |
| **Mistral Small 3.1**| 78.4% | 76.2% | 52.1% | 128k |
| **Ministral 8B** | 71.2% | 68.5% | 24.5% | 128k |
| **Mixtral 8x22B** | 77.3% | 76.6% | 38.4% | 64k |

---

## 4. Licensing Terms

Mistral AI maintains a dual-licensing strategy to balance ecosystem growth with commercial sustainability.

| License Type | Scope | Models Covered |
| :--- | :--- | :--- |
| **Apache 2.0** | Permissive: Free for research and commercial use. | Mistral Large 3, Ministral 3, Mistral Small 3.1, Mixtral 8x22B, Mistral Nemo. |
| **Research License**| Free for non-commercial research; paid for large commercial entities. | Mistral Large 2 (Legacy flagship). |
| **Proprietary** | Accessible via Mistral API or custom enterprise agreements. | Magistral Medium, Mistral Optimized (Domain Specific). |
| **Creative Commons**| Non-commercial use. | Voxtral TTS (Base weights). |

**Note:** Organizations with annual revenue exceeding $20M typically require a commercial agreement for models released under the Mistral Research License if used for commercial deployment.

---

## 5. Summary for Implementation
For team tasks involving Mistral integration:
1.  **High-Performance/Scale:** Use **Mistral Large 3** via API or on-prem H100 clusters (Apache 2.0).
2.  **Edge/Local:** Use **Ministral 3** family for privacy-first or offline applications.
3.  **Reasoning Tasks:** Leverage **Magistral Small** for complex logical derivation tasks.
4.  **Coding Agents:** **Devstral 2** is the preferred model for autonomous repository-level engineering.
