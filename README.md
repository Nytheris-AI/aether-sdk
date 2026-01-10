# Aether SDK

**Production-ready AI Context Engineering SDK**

The **Aether SDK** is a protocol-based library designed to help you build "AI Brains" that are modular, scalable, and independent of specific infrastructure providers.

It provides a robust **Memory Manager** that orchestrates:
*   **Ingestion**: Parsing -> Chunking -> Embedding -> Extraction -> Storage.
*   **Recall**: Hybrid Search (Vector + Graph).
*   **Intelligence**: Context Distillation and Q&A.

## 🚀 Features

*   **Zero Vendor Lock-in**: Define your own adapters for LLMs, Vector DBs, and Graph DBs.
*   **Protocol-First Architecture**: Strictly typed protocols (`LLMAdapterProtocol`, `VectorStoreProtocol`) ensure you own your stack.
*   **Hybrid Search**: Semantically configured retrieval strategies.
*   **Production Ready**: Heavy focus on type safety, clean interfaces, and testing.

## 📦 Installation

```bash
uv add aether-sdk
# or
pip install aether-sdk
```

## ⚡ Quickstart

Initialize the `MemoryManager` with your chosen adapters.

```python
import asyncio
from aether_sdk.memory import MemoryManager
from aether_sdk.adapters.openai import OpenAIAdapter
from aether_sdk.adapters.qdrant import QdrantAdapter

async def main():
    # 1. Initialize Adapters
    # You provide the drivers. We provide the logic.
    llm = OpenAIAdapter(api_key="sk-...")
    vectors = QdrantAdapter(url="http://localhost:6333", collection_name="memory")

    # 2. Initialize Manager
    brain = MemoryManager(
        project_id="my-project",
        llm_adapter=llm,
        embedding_adapter=llm, # OpenAI handles both
        vector_store=vectors
    )

    # 3. Ingest Data
    await brain.ingest_text(
        "Aether SDK allows you to swap database providers easily.",
        filename="overview.txt"
    )

    # 4. Ask Questions
    answer = await brain.ask("What is the benefit of Aether SDK?")
    print(answer)

if __name__ == "__main__":
    asyncio.run(main())
```

## 🏗️ Architecture

The SDK is built on a **Resources** layer that handles the complexity of context engineering:

1.  **IngestionResource**: Handles document parsing (via `docling`), chunking, and graph extraction.
2.  **RetrievalResource**: Manages vector similarity search and graph traversal.
3.  **DistillationResource**: Synthesizes high-density answers from retrieved context.

You simply inject the **Infrastructure Adapters**, and the **MemoryManager** wires these resources together.

## 🛡️ License

MIT
