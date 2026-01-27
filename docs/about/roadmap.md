# Aether SDK Roadmap

The Aether SDK is evolving from a research prototype into a robust, production-ready infrastructure for AI context engineering. Our mission is to provide developers with a sovereign, high-performance memory layer that bridge the gap between simple text retrieval and human-like cognitive memory.

---

## 🗺️ Product Milestones

### Phase 1: Core Foundation
**Goal:** Establish a robust protocol-driven architecture for vendor-neutral integrations.

*   **Finalize Protocol Layer:** Ensure clean, stable interfaces for LLMs, Vector Stores, and Knowledge Graphs.
*   **Agnostic Adapters:** Implement reference adapters for OpenAI, Anthropic, Qdrant, Chroma, and Neo4j.
*   **Sovereign Mode:** Enable strictly local execution using Ollama and local storage for air-gapped security.

### Phase 2: Knowledge Ingestion & Structuring
**Goal:** Transform raw data into a structured knowledge base with high semantic density.

*   **Advanced Parsing:** Integrate industry-standard parsers (e.g., `docling`) for complex document types (PDF, XLSX, etc.).
*   **Autonomous Graph Extraction:** Implement automatic entity and relationship extraction to build a persistent Knowledge Graph.
*   **Entity Resolution:** Build intelligent mechanisms to merge duplicate entities and maintain graph integrity.

### Phase 3: Hybrid Retrieval & Context Optimization
**Goal:** Deliver the most accurate context with minimal token overhead.

*   **Hybrid Retrieval Engine:** Combine semantic vector search with graph traversal (inspired by HippoRAG) for multi-hop reasoning.
*   **Context Distillation:** Implement active context window optimization to reduce token costs while preserving information density.
*   **Recursive Consolidation:** Develop background "sleep cycles" to consolidate episodic memories into long-term structured knowledge.

---

## 🛠 Features & Capabilities

### 1. Protocol & Architecture
*   **LLM Integration:** Standardized interface for any completion provider.
*   **Storage Abstraction:** Unified CRUD operations for both Vector and Graph databases.
*   **Resource Orchestration:** High-level handlers for memory lifecycle management.

### 2. Intelligent Ingestion
*   **Multi-Format Parsing:** Support for a wide range of data sources and document formats.
*   **Semantic Atomization:** Converting narrative context into atomic, indexable facts.
*   **Schema Evolution:** Handling changes in data structure without losing historical context.

### 3. Advanced Memory Operations
*   **Vector search (Episodic):** Local and cloud-scale vector retrieval.
*   **Graph Traversal (Semantic):** Relational reasoning over extracted knowledge.
*   **High-Throughput Buffer:** Asynchronous ingestion for high-scale applications.

---

## ⚠️ Development Focus

We are currently prioritizing:
*   **Developer Experience (DX):** Simplifying the integration of persistent memory into existing agentic frameworks.
*   **Performance:** Minimizing latency in hybrid retrieval pipelines.
*   **Accuracy:** Reducing hallucinations by providing higher-quality, structured context.

---
*Last Updated: January 2026*