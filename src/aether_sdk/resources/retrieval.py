from typing import List, Dict, Any, Optional
from aether_sdk.core.protocols import VectorStoreProtocol, GraphStoreProtocol, EmbeddingAdapterProtocol

class RetrievalResource:
    """
    Handles context retrieval (Search layer).
    """
    def __init__(
        self,
        project_id: str,
        vector_store: VectorStoreProtocol,
        embedding_adapter: EmbeddingAdapterProtocol,
        graph_store: Optional[GraphStoreProtocol] = None,
    ):
        self.project_id = project_id
        self._vectors = vector_store
        self._graph = graph_store
        self._embeddings = embedding_adapter

    async def retrieve_hybrid(self, query: str, top_k: int = 5) -> Dict[str, Any]:
        """
        Performs retrieval. Currently just Vector Search.
        """
        # 1. Embed Query
        query_vector = self._embeddings.embed_text(query)
        
        # 2. Vector Search
        # Note: In a real implementation, we would filter by project_id here
        results = self._vectors.search(query_vector, top_k=top_k)
        
        # 3. Format Results
        chunks = []
        for _, score, metadata in results:
            if metadata.get("project_id") == self.project_id:
                chunks.append(metadata.get("content", ""))
                
        # 4. Graph Traversal (Placeholder)
        graph_context = []
        
        return {
            "chunks": chunks,
            "graph_context": graph_context,
            "mode": "hybrid" if self._graph else "vector"
        }
