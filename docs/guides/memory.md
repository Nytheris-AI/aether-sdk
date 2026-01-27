# AgentMemory System

The `AgentMemory` system provides agents with a multi-layered memory architecture, inspired by human cognitive patterns.

## Memory Layers

### 1. Working Memory (Short-Term)
- **Implementation**: Volatile RAM (`List[MemoryItem]`).
- **Use Case**: Holding immediate conversation context, temporary observations.

### 2. Episodic Memory (Long-Term Semantic)
- **Implementation**: Vector Database (`VectorStoreProtocol`).
- **Use Case**: Retrieving relevant past experiences based on semantic similarity.

### 3. Semantic Memory (Long-Term Knowledge)
- **Implementation**: Knowledge Graph (`GraphStoreProtocol` or integrations like **Graphiti**).
- **Use Case**: Storing absolute facts and complex relationships that don't depend on specific episodes.

## Usage Example

```python
memory = AgentMemory(
    project_id="agent-007",
    vector_store=qdrant_adapter,
    embedding_adapter=openai_embedder,
    graph_store_client=graphiti_client # Integration with Graphiti
)

# Store an event
await memory.add_episodic("User mentioned they like coffee.", metadata={"source": "chat"})

# Store a fact
await memory.add_semantic("User is allergic to peanuts.")

# Retrieve context for a query
past_memories = await memory.retrieve("What does the user like to drink?")
```

## Consolidation
The `consolidate()` method (WIP) is designed to move important insights from working memory into long-term storage (Episodic or Semantic) based on significance and repetition.
