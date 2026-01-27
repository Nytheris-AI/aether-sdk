# SDK Usage Guide

This guide shows how to set up and use the Aether SDK for common use cases.

## Basic Setup (LiteLLM + Qdrant)

```python
import asyncio
from aether_sdk.adapters.litellm import LiteLLMAdapter
from aether_sdk.adapters.qdrant import QdrantAdapter
from aether_sdk.resources.brain import BrainResource
from aether_sdk.resources.ingestion import IngestionResource
from aether_sdk.resources.retrieval import RetrievalResource
from aether_sdk.resources.distillation import DistillationResource
from aether_sdk.types.shared import Document

async def main():
    # 1. Initialize Adapters
    llm = LiteLLMAdapter(model="gpt-4", api_key="sk-...")
    vector_db = QdrantAdapter(location=":memory:") # Local in-memory Qdrant
    
    # 2. Build Ingestion Pipeline
    ingestion = IngestionResource(
        project_id="demo",
        vector_store=vector_db,
        embedding_adapter=llm # LiteLLM can act as embedder if configured
    )
    
    # 3. Ingest Data
    doc = Document(id="doc1", content="The Aether SDK is built by Nytheris.", filename="info.txt")
    await ingestion.ingest_document(doc)
    
    # 4. Setup Retrieval and Brain
    retriever = RetrievalResource(project_id="demo", vector_store=vector_db, embedding_adapter=llm)
    distiller = DistillationResource(llm_adapter=llm)
    brain = BrainResource(retrieval_resource=retriever, distillation_resource=distiller)
    
    # 5. Query the Brain
    answer = await brain.generate_answer("Who built the Aether SDK?")
    print(f"Answer: {answer}")

asyncio.run(main())
```

## Using OpenRouter

```python
llm = LiteLLMAdapter(
    model="openrouter/anthropic/claude-3-opus",
    api_key="sk-or-...",
    site_url="https://your-app.com",
    app_name="Demo App"
)
```

## Adding Memory to an Agent

```python
from aether_sdk.memory_system import AgentMemory

memory = AgentMemory(
    project_id="demo-agent",
    vector_store=vector_db,
    embedding_adapter=llm
)

await memory.add_episodic("The user prefers dark mode.")
relevant = await memory.retrieve("What are the user preferences?")
```
