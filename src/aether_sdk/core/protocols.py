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

    def generate_structured(
        self,
        messages: List[Dict[str, str]],
        response_schema: Dict[str, Any],
        **kwargs: Any,
    ) -> Dict[str, Any]:
        """Generate structured JSON output.
        
        Args:
            messages: Chat messages.
            response_schema: JSON Schema or Pydantic model dict (implementation specific).
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
    
    def initialize(self) -> None:
        """Initialize the vector store (e.g., create tables/collections)."""
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

    def upsert_node(
        self,
        project_id: str,
        name: str,
        type: str,
        properties: Optional[Dict[str, Any]] = None
    ) -> None:
        """Upsert a node (Entity)."""
        ...

    def upsert_relation(
        self,
        project_id: str,
        source: str,
        target: str,
        type: str,
        properties: Optional[Dict[str, Any]] = None
    ) -> None:
        """Upsert a relation (Edge)."""
        ...
@runtime_checkable
class TemporalGraphProtocol(Protocol):
    """Protocol for Temporal Knowledge Graph storage (e.g., Graphiti)."""
    
    async def initialize(self) -> None:
        """Initialize the graph store (e.g., build indices)."""
        ...
        
    async def close(self) -> None:
        """Close the graph store connection."""
        ...
        
    async def add_episode(self, content: str, source_id: str = "user") -> None:
        """Add a temporal episode to the graph."""
        ...
        
    async def add_triplet(self, source_node: Any, edge: Any, target_node: Any) -> None:
        """Add a manual fact triple."""
        ...
        
    async def search(self, query: str) -> List[Dict[str, Any]]:
        """Perform a hybrid search on the graph."""
        ...
