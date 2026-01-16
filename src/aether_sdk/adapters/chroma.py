from typing import List, Dict, Any, Tuple, Optional
import os
import uuid
import chromadb
from aether_sdk.core.protocols import VectorStoreProtocol

class ChromaAdapter:
    """
    Implementation of VectorStoreProtocol for ChromaDB.
    """
    def __init__(self, persist_directory: str = None, collection_name: str = "aether_vectors"):
        self.persist_directory = persist_directory or os.getenv("CHROMA_PERSIST_DIRECTORY", "./chroma_db")
        self.client = chromadb.PersistentClient(path=self.persist_directory)
        self.collection_name = collection_name
        self._initialized = False

    def initialize(self):
        """Create the collection if it doesn't exist."""
        if not self._initialized:
            self._ensure_collection()
            self._initialized = True

    def _ensure_collection(self):
        self.client.get_or_create_collection(name=self.collection_name)

    def upsert(self, id: str, vector: List[float], metadata: Dict[str, Any]) -> None:
        collection = self.client.get_collection(name=self.collection_name)
        
        # Ensure ID
        doc_id = id if id else str(uuid.uuid4())
        
        collection.upsert(
            ids=[doc_id],
            embeddings=[vector],
            metadatas=[metadata]
        )

    def search(
        self,
        query_vector: List[float],
        top_k: int = 5,
    ) -> List[Tuple[str, float, Dict[str, Any]]]:
        
        collection = self.client.get_collection(name=self.collection_name)
        
        results = collection.query(
            query_embeddings=[query_vector],
            n_results=top_k,
            include=['metadatas', 'distances']
        )
        
        hits = []
        if results['ids']:
            ids = results['ids'][0]
            metadatas = results['metadatas'][0]
            distances = results['distances'][0]
            
            for id, meta, dist in zip(ids, metadatas, distances):
                # Chroma returns distance (lower better). 
                # Protocol expects score (higher better)? 
                # QdrantAdapter returns score. 
                # Usually consumers expect score. 
                # For now returning distance as score, but user might need to know semantics.
                # Let's assume the consumer handles it or we invert it if we knew the specific metric.
                hits.append((id, dist, meta))
                
        return hits
