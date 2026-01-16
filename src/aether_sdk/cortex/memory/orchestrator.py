from enum import Enum
from typing import List, Dict, Any, Optional, Tuple
from dataclasses import dataclass, field
from datetime import datetime
import uuid
import inspect
import logging
import asyncio

from aether_sdk.core.protocols import VectorStoreProtocol, EmbeddingAdapterProtocol, TemporalGraphProtocol
from aether_sdk.types.shared import Document

class MemoryType(Enum):
    EPISODIC = "episodic"
    SEMANTIC = "semantic"
    WORKING = "working"

@dataclass
class MemoryItem:
    content: str
    memory_type: MemoryType
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    created_at: datetime = field(default_factory=datetime.now)
    significance: float = 1.0
    metadata: Dict[str, Any] = field(default_factory=dict)
    embedding: Optional[List[float]] = None

from aether_sdk.cortex.memory.buffer import IngestionBuffer
from aether_sdk.cortex.memory.worker import WorkerPool
from aether_sdk.cortex.memory.cache import HotContextCache

class AgentMemory:
    """
    Unified Memory Orchestrator.
    Manages coordination between Vector Store (Episodic) and Graphiti (Semantic).
    Supports high-throughput asynchronous consolidation and fast hybrid retrieval.
    """
    
    def __init__(
        self, 
        project_id: str,
        vector_store: VectorStoreProtocol,
        embedding_adapter: EmbeddingAdapterProtocol,
        graph_store_client: Optional[TemporalGraphProtocol] = None, # Graphiti Client
        buffer_capacity: int = 1000,
        num_workers: int = 2,
        cache_ttl: int = 300
    ):
        self.project_id = project_id
        self.vector_store = vector_store
        self.embedding_adapter = embedding_adapter
        self.graph_store = graph_store_client
        self.working_memory: List[MemoryItem] = []
        
        # 1. Initialize Ingestion Pipeline
        self.buffer = IngestionBuffer(capacity=buffer_capacity)
        self.worker_pool = WorkerPool(self.buffer, self, num_workers=num_workers)
        
        # 2. Initialize Retrieval Cache
        self.cache = HotContextCache(ttl=cache_ttl)
        
        # 3. Initialization management
        self._workers_started = False
        self._start_lock = asyncio.Lock()

    async def start(self):
        """Start the background consolidation workers."""
        async with self._start_lock:
            if not self._workers_started:
                logging.info(f"AgentMemory: Starting components (Graph Store: {self.graph_store is not None})")
            
            # Initialize Vector Store
            if hasattr(self.vector_store, "initialize"):
                logging.info("AgentMemory: Initializing Vector Store...")
                res = self.vector_store.initialize()
                if inspect.isawaitable(res):
                    await res
            
            # Initialize Graph Store if it has an initialization step (e.g., Graphiti indices)
            if self.graph_store:
                if hasattr(self.graph_store, "initialize"):
                    logging.info("AgentMemory: Initializing Graph Store...")
                    await self.graph_store.initialize()
                else:
                    logging.info("AgentMemory: Graph Store has no 'initialize' method.")
                
            await self.worker_pool.start()
            self._workers_started = True
            logging.info("AgentMemory: Consolidation workers started.")

    async def stop(self):
        """Stop background workers and close stores."""
        if self._workers_started:
            await self.worker_pool.stop()
            self._workers_started = False
            
            # Close Graph Store if needed
            if self.graph_store and hasattr(self.graph_store, "close"):
                await self.graph_store.close()
                
            logging.info("AgentMemory: Consolidation workers stopped.")

    async def add_memory(self, content: str, metadata: Dict[str, Any] = None) -> str:
        """
        Unified entry point for adding memory.
        Pushes to the high-throughput buffer for asynchronous consolidation.
        """
        metadata = metadata or {}
        
        # Ensure workers are running
        if not self._workers_started:
            await self.start()
            
        # 1. Push to buffer (Immediate ACK)
        pushed = await self.buffer.push(content, metadata)
        
        if not pushed:
             # Fallback to sync if buffer is full (Backpressure handling)
             logging.warning("AgentMemory: Ingestion buffer full, falling back to synchronous consolidation.")
             return await self._consolidate_immediate(content, metadata)
             
        # Return a temporary ID or ACK
        return "ak-" + str(uuid.uuid4())[:8]

    async def _consolidate_immediate(self, content: str, metadata: Dict[str, Any]) -> str:
        """Internal synchronous consolidation path (used by workers)."""
        # Add to Episodic (Vector)
        mem_id = await self.add_episodic(content, metadata)
        # Add to Semantic (Graphiti)
        await self.add_semantic(content)
        return mem_id

    async def add_episodic(self, content: str, metadata: Dict[str, Any] = None) -> str:
        """Adds a memory to the Episodic layer (Vector Store)."""
        metadata = metadata or {}
        metadata["type"] = MemoryType.EPISODIC.value
        metadata["created_at"] = datetime.now().isoformat()
        metadata["content"] = content
        
        item = MemoryItem(
            content=content,
            memory_type=MemoryType.EPISODIC,
            metadata=metadata
        )
        
        # Generate embedding
        embedding = self.embedding_adapter.embed_text(content)
        item.embedding = embedding
        
        # Store in Vector DB
        res = self.vector_store.upsert(id=item.id, vector=embedding, metadata=metadata)
        if inspect.isawaitable(res):
            await res
        
        return item.id

    async def add_semantic(self, fact: str, source: str = "user") -> None:
        """Adds a fact to the Semantic layer (Graphiti)."""
        if self.graph_store:
            try:
                # Graphiti autonomous extraction
                await self.graph_store.add_episode(content=fact, source_id=source)
            except Exception as e:
                logging.error(f"Graphiti error during semantic add: {e}")

    async def retrieve(self, query: str, limit: int = 5) -> List[MemoryItem]:
        """
        Hybrid retrieval from all memory layers with caching.
        Performs parallel search across Vector and Graph stores.
        """
        # 1. Check Hot Cache
        cache_key = f"{self.project_id}:{query}:{limit}"
        cached_results = self.cache.get(cache_key)
        if cached_results:
            return cached_results

        # 2. Parallel Search (Using asyncio.gather for high-scale throughput)
        search_tasks = [self._search_vector(query, limit)]
        
        if self.graph_store:
            search_tasks.append(self._search_graph(query))
            
        # Execute in parallel
        task_results = await asyncio.gather(*search_tasks)
        
        # Flatten results
        results = []
        for task_res in task_results:
            results.extend(task_res)

        # 3. Reciprocal Rank Fusion (RRF) Ranking
        # RRF formula: Score = sum(1 / (k + rank))
        k = 60
        rrf_scores = {}
        content_map = {} # content -> list of MemoryItems
        
        # Process each result set
        for result_set in task_results:
            for rank, item in enumerate(result_set, 1):
                content = item.content
                if content not in rrf_scores:
                    rrf_scores[content] = 0.0
                    content_map[content] = []
                
                rrf_scores[content] += 1.0 / (k + rank)
                content_map[content].append(item)

        # 4. Sort and Format Final Results
        # Sort content by RRF score descending
        sorted_content = sorted(rrf_scores.keys(), key=lambda x: rrf_scores[x], reverse=True)
        
        final_results = []
        for content in sorted_content:
            # For each content, prefer Semantic (Graph) metadata if available
            items = content_map[content]
            best_item = next((i for i in items if i.memory_type == MemoryType.SEMANTIC), items[0])
            
            # Attach RRF score to metadata for transparency
            best_item.metadata["rrf_score"] = rrf_scores[content]
            final_results.append(best_item)
            
        final_results = final_results[:limit]
        
        # 5. Populate Cache
        self.cache.set(cache_key, final_results)
        
        return final_results

    async def _search_vector(self, query: str, limit: int) -> List[MemoryItem]:
        query_vector = self.embedding_adapter.embed_text(query)
        search_res = self.vector_store.search(query_vector=query_vector, top_k=limit)
        
        if inspect.isawaitable(search_res):
            vector_results = await search_res
        else:
            vector_results = search_res
            
        logging.info(f"AgentMemory: Vector Search found {len(vector_results or [])} results.")
        items = []
        for vid, score, meta in vector_results:
            items.append(MemoryItem(
                id=vid,
                content=meta.get("content", ""),
                memory_type=MemoryType.EPISODIC,
                significance=score,
                metadata=meta
            ))
        return items

    async def _search_graph(self, query: str) -> List[MemoryItem]:
        try:
            graph_results = await self.graph_store.search(query)
            
            logging.info(f"AgentMemory: Graph Search found {len(graph_results or [])} results.")
            items = []
            if isinstance(graph_results, list):
                 for res in graph_results:
                     # Handle both objects and dictionaries
                     if isinstance(res, dict):
                         content = res.get("fact", res.get("text", str(res)))
                     else:
                         content = getattr(res, "fact", getattr(res, "text", str(res)))
                         
                     items.append(MemoryItem(
                         content=content,
                         memory_type=MemoryType.SEMANTIC,
                         metadata={"source": "graphiti"}
                     ))
            return items
        except Exception as e:
            logging.error(f"Graphiti search error: {e}")
            return []
