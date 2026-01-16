from typing import Any, Dict, List, Optional
import uuid
import time
import inspect

class LazyMemoryStore:
    """
    Implements the 'Ecphory' style Lazy Graph.
    - Entities -> Vector DB (Qdrant)
    - Content -> Metadata Store (Postgres/JSON)
    """
    
    def __init__(self, vector_store: Any, embeddings: Any, metadata_store: Any = None):
        self.vector_store = vector_store
        self.embeddings = embeddings
        self.metadata_store = metadata_store or {} # Fallback to dict for MVP
        
    async def add_memory(self, content: str, entities: List[str]) -> str:
        """
        Stores memory. 
        1. Generates UUID.
        2. Stores full content in Metadata Store.
        3. Embeds and stores Entities in Vector Store pointing to UUID.
        """
        mem_id = str(uuid.uuid4())
        timestamp = time.time()
        
        # 1. Store Metadata
        entry = {
            "id": mem_id,
            "content": content,
            "entities": entities,
            "timestamp": timestamp
        }
        
        # Simulating DB write
        if isinstance(self.metadata_store, dict):
            self.metadata_store[mem_id] = entry
        elif hasattr(self.metadata_store, "insert"): # Mock DB interface
            await self.metadata_store.insert(entry)
            
        # 2. Store Vectors (Entity Pointers)
        # We embed the ENTITY text itself.
        
        if entities:
            # Generate embeddings for all entities in batch
            try:
                # Adapters might be async or just wrapped in async
                vectors = await self.embeddings.embed_batch(entities)
            except Exception as e:
                # Fallback if batch fails or adapter is sync
                 print(f"Embedding error: {e}")
                 vectors = []

            for i, entity in enumerate(entities):
                 if hasattr(self.vector_store, "upsert"):
                     if i < len(vectors):
                         vector = vectors[i]
                         res = self.vector_store.upsert(
                             id=str(uuid.uuid4()), # Unique vector ID
                             vector=vector,
                             metadata={"mem_id": mem_id, "type": "entity_pointer", "entity_text": entity}
                         )
                         if inspect.isawaitable(res):
                             await res
                 # elif fallback removed for simplicity/protocol compliance
                 
        return mem_id

    async def retrieve(self, query_entities: List[str]) -> List[Dict[str, Any]]:
        """
        Retrieves memories associated with the query entities.
        1. Vector Search for each entity.
        2. Collect mem_ids.
        3. Fetch full content from Metadata Store.
        """
        mem_ids = set()
        
        # 1. Search (Embed query entities first)
        if query_entities:
            query_vectors = await self.embeddings.embed_batch(query_entities)
            
            for i, entity in enumerate(query_entities):
                results = []
                if hasattr(self.vector_store, "search") and i < len(query_vectors):
                    # Correct Protocol: search(query_vector, top_k)
                    res = self.vector_store.search(
                        query_vector=query_vectors[i], 
                        top_k=5
                    )
                    if inspect.isawaitable(res):
                        results = await res
                    else:
                        results = res
                
                # Extract mem_ids from results
                for res in results:
                    # Expecting tuple (id, score, metadata) per Protocol
                    if isinstance(res, tuple) and len(res) >= 3:
                        meta = res[2]
                        if "mem_id" in meta:
                            mem_ids.add(meta["mem_id"])
                    # Fallback for non-compliant adapters returning object/dict
                    elif isinstance(res, dict):
                         meta = res.get("metadata", {})
                         if "mem_id" in meta:
                            mem_ids.add(meta["mem_id"])
        
        # 2. Fetch
        memories = []
        for mid in mem_ids:
            if isinstance(self.metadata_store, dict):
                if mid in self.metadata_store:
                    memories.append(self.metadata_store[mid])
            elif hasattr(self.metadata_store, "get"):
                 item = await self.metadata_store.get(mid)
                 if item: memories.append(item)
            
        return memories
