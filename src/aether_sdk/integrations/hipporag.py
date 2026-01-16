from typing import List, Dict, Any, Tuple
# We do not strictly import hipporag here to avoid hard dependency at module level
# but the protocols are conceptually aligned.

from aether_sdk.core.protocols import LLMAdapterProtocol, EmbeddingAdapterProtocol, VectorStoreProtocol

class AetherToHippoLLM:
    """
    Adapts Aether SDK LLM Protocol to HippoRAG LLM Protocol.
    """
    def __init__(self, adapter: LLMAdapterProtocol):
        self.adapter = adapter
        
    def infer(self, chat: List[Dict[str, str]], **kwargs) -> Tuple[str, Dict[str, Any], bool]:
        """
        HippoRAG expects: (response_str, metadata_dict, cache_hit_bool)
        Aether provides: generate_completion(messages) -> str
        """
        response = self.adapter.generate_completion(messages=chat, **kwargs)
        # Mock metadata since Aether doesn't standardize it yet
        metadata = {
            "finish_reason": "stop",
            "prompt_tokens": 0,
            "completion_tokens": 0
        }
        return response, metadata, False

class AetherToHippoEmbedding:
    """
    Adapts Aether SDK Embedding Protocol to HippoRAG Embedding Protocol.
    """
    def __init__(self, adapter: EmbeddingAdapterProtocol):
        self.adapter = adapter
        
    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        return self.adapter.embed_batch(texts)

class AetherToHippoVectorStore:
    """
    Adapts Aether SDK VectorStore Protocol to HippoRAG VectorStorage Protocol.
    """
    def __init__(self, adapter: VectorStoreProtocol):
        self.adapter = adapter

    def upsert(self, id: str, vector: List[float], metadata: Dict[str, Any]) -> None:
        self.adapter.upsert(id=id, vector=vector, metadata=metadata)
        
    def search(self, query_vector: List[float], top_k: int) -> List[Tuple[str, float, Dict[str, Any]]]:
        return self.adapter.search(query_vector=query_vector, top_k=top_k)

def create_hippo_engine(
    llm: LLMAdapterProtocol,
    embedding: EmbeddingAdapterProtocol,
    vector_store: VectorStoreProtocol
):
    """
    Factory to create a HippoEngine instance using Aether resources.
    Requires 'hipporag' to be installed.
    """
    try:
        from hipporag.engine import HippoEngine
    except ImportError:
        raise ImportError("HippoRAG is not installed. Please install 'hipporag' to use this integration.")
        
    return HippoEngine(
        llm=AetherToHippoLLM(llm),
        embeddings=AetherToHippoEmbedding(embedding),
        vector_storage=AetherToHippoVectorStore(vector_store)
    )
