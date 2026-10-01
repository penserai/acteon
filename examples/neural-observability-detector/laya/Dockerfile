FROM python:3.12-slim

ARG LAYA_VERSION=0.3.23
ARG TORCH_VERSION=2.14.0

RUN pip install --no-cache-dir "torch==${TORCH_VERSION}" \
      --index-url https://download.pytorch.org/whl/cpu \
    && pip install --no-cache-dir \
      "transformers==5.18.0" \
      "huggingface-hub==1.33.0" \
      "safetensors==0.8.0" \
      "numpy==2.5.3" \
    && pip install --no-cache-dir "laya[serve]==${LAYA_VERSION}"

EXPOSE 8000

ENTRYPOINT ["laya-serve"]
