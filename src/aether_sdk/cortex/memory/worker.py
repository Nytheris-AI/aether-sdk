import asyncio
import logging
from typing import Any, Dict, List, Optional
from aether_sdk.core.protocols import TemporalGraphProtocol
from aether_sdk.cortex.memory.buffer import IngestionBuffer

class MemoryWorker:
    """
    Background worker that processes memories from the IngestionBuffer.
    Consolidates raw content into Episodic (Vector) and Semantic (Graph) layers.
    """
    
    def __init__(self, worker_id: int, buffer: IngestionBuffer, orchestrator: Any):
        self.worker_id = worker_id
        self.buffer = buffer
        self.orchestrator = orchestrator
        self.is_running = False
        self._task = None

    async def start(self):
        """Start the worker loop."""
        self.is_running = True
        self._task = asyncio.create_task(self._loop())
        logging.info(f"MemoryWorker-{self.worker_id} started.")

    async def stop(self):
        """Stop the worker loop."""
        self.is_running = False
        if self._task:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
        logging.info(f"MemoryWorker-{self.worker_id} stopped.")

    async def _loop(self):
        while self.is_running:
            try:
                # 1. Pull from buffer (blocks if empty)
                payload = await self.buffer.get_next()
                
                content = payload.get("content")
                metadata = payload.get("metadata", {})
                
                logging.debug(f"Worker-{self.worker_id} processing: {content[:20]}...")
                
                # 2. Consolidate (Episodic + Semantic)
                # We call the internal consolidation path of the orchestrator
                await self.orchestrator._consolidate_immediate(content, metadata)
                
                # 3. Mark as done (if buffer uses Task mapping, though our basic buffer doesn't yet)
                # self.buffer.queue.task_done()
                
            except Exception as e:
                logging.error(f"Worker-{self.worker_id} encountered error: {e}")
                # Brief pause to prevent infinite fast-fail loops
                await asyncio.sleep(1)

class WorkerPool:
    """
    Manages a pool of MemoryWorkers.
    """
    
    def __init__(self, buffer: IngestionBuffer, orchestrator: Any, num_workers: int = 2):
        self.buffer = buffer
        self.orchestrator = orchestrator
        self.num_workers = num_workers
        self.workers = []

    async def start(self):
        for i in range(self.num_workers):
            worker = MemoryWorker(i, self.buffer, self.orchestrator)
            await worker.start()
            self.workers.append(worker)

    async def stop(self):
        tasks = [w.stop() for w in self.workers]
        await asyncio.gather(*tasks)
        self.workers = []
