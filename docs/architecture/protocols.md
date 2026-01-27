# Core Protocols

The Aether SDK is built on the principle of **Inversion of Control**. Core logic depends on abstract protocols rather than concrete implementations.

## LLMAdapterProtocol

Defines how the system interacts with Large Language Models.

```python
class LLMAdapterProtocol(Protocol):
    def generate_completion(self, messages: List[Dict[str, str]], **kwargs: Any) -> str:
        """Generates a text completion."""
        ...

    def generate_structured(self, messages: List[Dict[str, str]], response_schema: Dict[str, Any], **kwargs: Any) -> Dict[str, Any]:
        """Generates JSON output conforming to a schema."""
        ...
```

## EmbeddingAdapterProtocol

Handles the conversion of text into numerical vectors.

```python
class EmbeddingAdapterProtocol(Protocol):
    def embed_text(self, text: str) -> List[float]:
        """Embeds a single prompt."""
        ...

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        """Embeds multiple texts efficiently."""
        ...
```

## VectorStoreProtocol

Interface for vector databases (semantic storage).

```python
class VectorStoreProtocol(Protocol):
    def upsert(self, id: str, vector: List[float], metadata: Dict[str, Any]) -> None:
        """Stores a vector with associated metadata."""
        ...

    def search(self, query_vector: List[float], top_k: int = 5) -> List[Tuple[str, float, Dict[str, Any]]]:
        """Finds most similar vectors."""
        ...
```

## GraphStoreProtocol

Interface for Graph databases (knowledge representation).

```python
class GraphStoreProtocol(Protocol):
    def execute_query(self, query: str, parameters: Optional[Dict[str, Any]] = None) -> List[Dict[str, Any]]:
        """Executes a native graph query (e.g., Cypher)."""
        ...

    def upsert_node(self, project_id: str, name: str, type: str, properties: Optional[Dict[str, Any]] = None) -> None:
        """Creates or updates an entity node."""
        ...

    def upsert_relation(self, project_id: str, source: str, target: str, type: str, properties: Optional[Dict[str, Any]] = None) -> None:
        """Creates or updates a relationship between nodes."""
        ...
```

## Why Protocols?

- **Vendor Independence**: Switch from OpenAI to Anthropic or from Qdrant to Chroma by just changing an adapter.
- **Testability**: Easily mock any protocol for unit testing.
- **Extensibility**: Users can implement their own adapters for proprietary systems.
