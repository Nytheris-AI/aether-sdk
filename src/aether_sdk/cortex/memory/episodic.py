from typing import List, Dict, Any, Optional
import uuid
import datetime

class EpisodicMemory:
    """
    Interface for Episodic Memory (Vector-based).
    Stores 'MemoryEntry' objects alike SimpleMem.
    """
    
    def __init__(self, vector_store: Any):
        self.vector_store = vector_store
        
    async def store_episode(self, content: str, metadata: Optional[Dict[str, Any]] = None) -> str:
        """Store a new episode."""
        if metadata is None:
            metadata = {}
            
        entry_id = str(uuid.uuid4())
        
        # Standardize metadata schema based on SimpleMem MemoryEntry
        full_metadata = {
            "source": "user",
            "timestamp": datetime.datetime.now().isoformat(),
            **metadata
        }
        
        # In a real impl, we'd chunk here or expect atomizer to have done it.
        # We assume vector_store handles embedding internally or via passed adapter (not shown here).
        # For protocols, usually: add(str, metadata, id)
        await self.vector_store.add([content], [full_metadata], [entry_id])
        
        return entry_id
        
    async def retrieve_episodes(self, query: str, limit: int = 5) -> List[Dict[str, Any]]:
        """Retrieve similar past episodes."""
        # returns List[Tuple[id, score, content, metadata]] usually
        results = await self.vector_store.search(query, top_k=limit)
        
        formatted = []
        for res in results:
            # abstracting difference between adapters
            # assume res is object with .content and .metadata or dict
            formatted.append({
                "content": res.content if hasattr(res, 'content') else str(res),
                "score": res.score if hasattr(res, 'score') else 0.0
            })
        return formatted

