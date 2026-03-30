# Gemma 3 Research Report

## Overview
Gemma 3 is Google's newest family of open-weight models, released in March 2025. Built on the same research as Gemini 2.0, it introduces native multimodal capabilities and a vastly expanded context window.

## Architecture
- **Model Type:** Decoder-only Transformer with Grouped-Query Attention (GQA).
- **Multimodal Capabilities:** The 4B, 12B, and 27B versions can natively process both text and images.
- **Context Length:** Supports up to 128K tokens for multimodal models.

## Model Sizes & Variants
| Size | Type | Context Window | Multimodal? |
| :--- | :--- | :--- | :--- |
| **270M** | Text-only | 32K | No |
| **1B** | Text-only | 32K | No |
| **4B** | Multimodal | 128K | Yes |
| **12B** | Multimodal | 128K | Yes |
| **27B** | Multimodal | 128K | Yes |

## Performance Benchmarks
- **Elo Rating:** Gemma 3 27B-IT achieved an Elo rating of **1338**, competing with much larger models.
- **MMLU-Pro:** The 27B model scored **67.5**, showing strong reasoning capabilities.
- **IFEval:** The 270M model set new records for its size class.

## License
Released under the **Gemma Terms of Use**, which is a permissive "open-weights" license allowing for responsible commercial use.

## Ecosystem & Tools
- **ShieldGemma 2:** A 4B parameter safety model built on Gemma 3 for filtering sensitive content.
- **Gemma Scope 2:** An interpretability suite to help researchers understand the internal mechanics of the models.
- **Global Support:** Pre-trained on over 140 languages.

