from typing import List, Dict, Any, Tuple, Optional
import os
from uuid import uuid4
from qdrant_client import QdrantClient
from qdrant_client.models import PointStruct, Distance, VectorParams, Filter, FieldCondition, MatchValue
from aether_sdk.core.protocols import VectorStoreProtocol

class QdrantAdapter:
    """
    Reference implementation for Qdrant.
    Implements VectorStoreProtocol.
    """
    def __init__(self, url: str = None, api_key: str = None, collection_name: str = "aether_memory"):
        if url == ":memory:" or (url is None and os.getenv("QDRANT_URL") is None):
            self.client = QdrantClient(location=":memory:")
        else:
            self.client = QdrantClient(
                url=url or os.getenv("QDRANT_URL"),
                api_key=api_key or os.getenv("QDRANT_API_KEY")
            )
        self.collection_name = collection_name
        self._ensure_collection()

    def _ensure_collection(self):
        if not self.client.collection_exists(self.collection_name):
            self.client.create_collection(
                collection_name=self.collection_name,
                vectors_config=VectorParams(size=384, distance=Distance.COSINE) # Default size assumption
            )

    def upsert(self, id: str, vector: List[float], metadata: Dict[str, Any]) -> None:
        point = PointStruct(
            id=str(uuid4()) if not id else id, # Qdrant prefers UUIDs usually, but str allowed
            vector=vector,
            payload=metadata
        )
        self.client.upsert(
            collection_name=self.collection_name,
            points=[point]
        )

    def search(
        self,
        query_vector: List[float],
        top_k: int = 5,
    ) -> List[Tuple[str, float, Dict[str, Any]]]:
        
        # Use query_points instead of search (which seems unavailable/deprecated)
        search_result = self.client.query_points(
            collection_name=self.collection_name,
            query=query_vector,
            limit=top_k,
            with_payload=True
        ).points
        
        return [
            (hit.id, hit.score, hit.payload) for hit in search_result
        ]
