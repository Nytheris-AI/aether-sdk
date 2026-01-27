# Functional Resources

Resources are the high-level functional blocks of the Aether SDK. They consume adapters and protocols to perform complex tasks.

## IngestionResource

Orchestrates the data entry pipeline.
- **Responsibilities**: Chunking, Embedding, Vector Upsert, KG Extraction.

```python
ingestion = IngestionResource(
    project_id="my-project",
    vector_store=qdrant_adapter,
    embedding_adapter=openai_embedder,
    extraction_resource=extractor_resource, # Optional
    graph_store=neo4j_adapter               # Optional
)
await ingestion.ingest_document(my_doc)
```

## RetrievalResource

Manages the context search logic.
- **Hybrid Search**: Combines vector similarity with graph traversal (when available).

```python
retriever = RetrievalResource(
    project_id="my-project",
    vector_store=qdrant_adapter,
    embedding_adapter=openai_embedder
)
context = await retriever.retrieve_hybrid("What are the project goals?")
```

## BrainResource

The primary interface for intelligence. It connects Retrieval to Distillation.

```python
brain = BrainResource(
    retrieval_resource=retriever,
    distillation_resource=distiller
)
answer = await brain.generate_answer("Who is the lead engineer?")
```

## ParsingResource

Handles file conversion.
- Supports PDF, HTML, Markdown, and more via `docling` or `parsing` adapters.

## Extraction & Distillation

- **ExtractionResource**: Uses LLMs to find entities and relations in raw text.
- **DistillationResource**: Uses LLMs to summarize retrieved context into a coherent answer.
