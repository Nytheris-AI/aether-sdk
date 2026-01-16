from typing import List, Dict, Any, Optional, Tuple
from hipporag.engine import HippoEngine
from aether_sdk.core.protocols import LLMAdapterProtocol, EmbeddingAdapterProtocol, VectorStoreProtocol

# -------------------------------------------------------------------------
# Internal Adapters (Isolated from HippoRAG codebase)
# -------------------------------------------------------------------------

class _HippoLLMAdapter:
    """Wraps Aether LLMAdapterProtocol to HippoLLM protocol."""
    def __init__(self, client: LLMAdapterProtocol):
        self.client = client
    
    def infer(self, chat: Optional[List[Dict[str, str]]] = None, messages: Optional[List[Dict[str, str]]] = None, **kwargs) -> Tuple[str, Dict[str, Any], bool]:
        msgs = chat if chat is not None else messages
        if msgs is None:
            raise ValueError("HippoRAG adapter requires 'chat' or 'messages' argument")
        
        response = self.client.generate_completion(messages=msgs, **kwargs)
        return response, {}, False

class _HippoEmbeddingAdapter:
    """Wraps Aether EmbeddingAdapterProtocol to HippoEmbedding protocol."""
    def __init__(self, client: EmbeddingAdapterProtocol):
        self.client = client
        
    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        return self.client.embed_batch(texts)
        
class _HippoVectorStoreAdapter:
    """Wraps Aether VectorStoreProtocol to HippoVectorStorage protocol."""
    def __init__(self, client: VectorStoreProtocol):
        self.client = client
        
    def upsert(self, id: str, vector: List[float], metadata: Dict[str, Any]) -> None:
        self.client.upsert(id=id, vector=vector, metadata=metadata)
        
    def search(self, query_vector: List[float], top_k: int) -> List[Tuple[str, float, Dict[str, Any]]]:
        return self.client.search(query_vector=query_vector, top_k=top_k)

# -------------------------------------------------------------------------
# Resource
# -------------------------------------------------------------------------

class HippoRAGResource:
    """
    Integrates HippoRAG engine into Aether SDK.
    Provides methods to ingest documents and retrieve context using Knowledge Graph + Personalized PageRank.
    """
    def __init__(
        self, 
        llm_adapter: LLMAdapterProtocol, 
        embedding_adapter: EmbeddingAdapterProtocol, 
        vector_store_adapter: VectorStoreProtocol
    ):
        """
        Initialize HippoRAG engine with Aether adapters.
        """
        self.engine = HippoEngine(
            llm=_HippoLLMAdapter(llm_adapter),
            embeddings=_HippoEmbeddingAdapter(embedding_adapter),
            vector_storage=_HippoVectorStoreAdapter(vector_store_adapter)
        )

    def ingest(self, text: str, doc_id: Optional[str] = None) -> None:
        """
        Ingest a document text into HippoRAG.
        This triggers:
        1. OpenIE extraction (Triples).
        2. Embedding generation (Chunks, Entities, Facts).
        3. Vector Store insertion.
        4. Internal Knowledge Graph update.
        """
        self.engine.index_document(text, doc_id=doc_id)

    def query(self, query_text: str, top_k: int = 5) -> List[str]:
        """
        Retrieve relevant chunks using HippoRAG's PPR algorithm.
        """
        return self.engine.retrieve(query_text, top_k=top_k)
