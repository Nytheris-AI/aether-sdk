from typing import Any, Dict, List, Optional
import asyncio
import logging
from datetime import datetime

class IngestionBuffer:
    """
    Decouples the chat loop from memory consolidation.
    Provides a high-throughput staging area for incoming memories.
    """
    
    def __init__(self, capacity: int = 1000):
        # In a real high-scale system, this would be Redis or Kafka.
        # For the SDK base implementation, we use an in-memory queue.
        self.queue = asyncio.Queue(maxsize=capacity)
        logging.info(f"IngestionBuffer initialized with capacity {capacity}")
        
    async def push(self, content: str, metadata: Optional[Dict[str, Any]] = None) -> bool:
        """
        Push memory content to the buffer for background processing.
        Returns True if successful, False if buffer is full.
        """
        payload = {
            "content": content,
            "metadata": metadata or {},
            "timestamp": datetime.now().isoformat()
        }
        
        try:
            # We use put_nowait or wait with timeout to ensure chat low latency
            self.queue.put_nowait(payload)
            logging.debug(f"Pushed content to buffer: {content[:20]}...")
            return True
        except asyncio.QueueFull:
            logging.warning("IngestionBuffer is full! Inbound memory may be delayed or dropped.")
            return False

    async def get_next(self) -> Dict[str, Any]:
        """Get the next item for processing. Blocks if empty."""
        return await self.queue.get()

    def qsize(self) -> int:
        return self.queue.qsize()
