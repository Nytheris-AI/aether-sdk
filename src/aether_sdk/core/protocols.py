from typing import Protocol, Any, List, Dict, Optional, Tuple, runtime_checkable

@runtime_checkable
class LLMAdapterProtocol(Protocol):
    """Protocol for LLM integrations.
    
    Users implement this for their chosen LLM provider.
    We don't care HOW they call the LLM, just that they can.
    """
    
    def generate_completion(
        self,
        messages: List[Dict[str, str]],
        **kwargs: Any,
    ) -> str:
        """Generate completion from messages.
        
        Implementation is user's responsibility.
        They handle: API calls, retries, rate limits, costs.
        We handle: Context assembly, memory retrieval.
        """
        ...

@runtime_checkable
class EmbeddingAdapterProtocol(Protocol):
    """Protocol for embedding generation.
    
    Users choose their embedding provider.
    """
    
    def embed_text(self, text: str) -> List[float]:
        """Generate embedding vector for text."""
        ...
    
    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        """Generate embeddings for multiple texts."""
        ...

@runtime_checkable
class VectorStoreProtocol(Protocol):
    """Protocol for vector storage.
    
    Users bring their own vector database.
    """
    
    def upsert(self, id: str, vector: List[float], metadata: Dict[str, Any]) -> None:
        """Store vector with metadata."""
        ...
    
    def search(
        self,
        query_vector: List[float],
        top_k: int = 5,
    ) -> List[Tuple[str, float, Dict[str, Any]]]:
        """Search for similar vectors.
        
        Returns:
            List of (id, score, metadata) tuples.
        """
        ...

@runtime_checkable
class GraphStoreProtocol(Protocol):
    """Protocol for Graph Database storage (Optional Extension).
    
    Used for Knowledge Graph capabilities.
    """
    
    def execute_query(self, query: str, parameters: Optional[Dict[str, Any]] = None) -> List[Dict[str, Any]]:
        """Execute a Cypher/Gremlin query."""
        ...
