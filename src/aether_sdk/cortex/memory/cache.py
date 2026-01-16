from typing import Any, Dict, List, Optional
import time
import logging

class HotContextCache:
    """
    Optimized in-memory cache for frequently accessed or recent context.
    In a high-scale production setting, this would wrap Redis.
    """
    
    def __init__(self, ttl: int = 300, max_size: int = 100):
        self.ttl = ttl
        self.max_size = max_size
        self.cache: Dict[str, Dict[str, Any]] = {}
        logging.info(f"HotContextCache initialized (TTL: {ttl}s, MaxSize: {max_size})")

    def get(self, key: str) -> Optional[Any]:
        """Retrieve item from cache if it exists and is not expired."""
        if key in self.cache:
            entry = self.cache[key]
            if time.time() - entry["timestamp"] < self.ttl:
                logging.debug(f"Cache hit for key: {key}")
                return entry["data"]
            else:
                # Expire
                del self.cache[key]
        return None

    def set(self, key: str, data: Any):
        """Store item in cache with current timestamp."""
        # Simple LRU-style eviction if max_size reached
        if len(self.cache) >= self.max_size:
            # Remove the oldest entry
            oldest_key = min(self.cache.keys(), key=lambda k: self.cache[k]["timestamp"])
            del self.cache[oldest_key]
            
        self.cache[key] = {
            "data": data,
            "timestamp": time.time()
        }
        logging.debug(f"Cache set for key: {key}")

    def invalidate(self, key: str):
        """Manually remove an item from the cache."""
        if key in self.cache:
            del self.cache[key]

    def clear(self):
        """Clear all entries."""
        self.cache.clear()
