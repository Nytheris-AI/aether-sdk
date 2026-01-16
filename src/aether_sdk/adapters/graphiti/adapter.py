from typing import Any, Dict, List, Optional
import logging
import inspect
import json

try:
    from graphiti_core import Graphiti
    from graphiti_core.nodes import EpisodeType
except ImportError:
    Graphiti = None
    EpisodeType = None
    logging.warning("graphiti_core not found. GraphitiAdapter will not function.")

class GraphitiAdapter:
    """
    Adapter for Graphiti (Temporal Knowledge Graph).
    """
    
    def __init__(self, uri: str = "neo4j://localhost:7687", user: str = "neo4j", password: str = "password"):
        self.uri = uri
        self.user = user
        self.password = password
        self.client = None
        self._initialized = False
        
    async def connect(self):
        """
        Async method to create and initialize the Graphiti client.
        Call this method after creating the adapter instance.
        """
        if Graphiti:
            try:
                # Create the Graphiti client
                logging.info(f"GraphitiAdapter: Connecting to {self.uri}")
                self.client = Graphiti(self.uri, self.user, self.password)
                
                # If Graphiti() returns an awaitable, await it
                if inspect.isawaitable(self.client):
                    self.client = await self.client
                
                logging.info(f"GraphitiAdapter: Connected to {self.uri}")
                self._initialized = True
            except Exception as e:
                logging.error(f"GraphitiAdapter: Failed to connect. {e}")
                self.client = None
                self._initialized = False
        else:
            logging.warning("GraphitiAdapter: graphiti_core not available")

    async def initialize(self):
        """
        Build indices and constraints. Required once.
        Supports multiple possible Graphiti initialization methods.
        """
        if not self._initialized:
            await self.connect()
            
        if self.client:
            try:
                # Check for and await common Graphiti initialization methods
                init_methods = [
                    "build_indices_and_constraints",
                    "init", 
                    "connect", 
                    "ensure_indices", 
                    "register_models"
                ]
                
                for method_name in init_methods:
                    if hasattr(self.client, method_name):
                        method = getattr(self.client, method_name)
                        logging.info(f"Graphiti: Found initialization method '{method_name}'. Executing...")
                        
                        # Call the method
                        res = method()
                        
                        # If it's a coroutine or returns an awaitable, await it
                        if inspect.isawaitable(res):
                            await res
                            logging.info(f"Graphiti: Successfully awaited '{method_name}'.")
                        else:
                            logging.info(f"Graphiti: '{method_name}' executed synchronously.")
                        
                        # Usually only need to call one initialization method
                        break
                
            except Exception as e:
                logging.error(f"Graphiti initialization error: {e}")
        else:
            logging.warning("Graphiti: Cannot initialize (Client not connected)")

    async def close(self):
        """
        Close the Graphiti connection.
        """
        if self.client:
            try:
                logging.info("Graphiti: Closing connection...")
                close_result = self.client.close()
                if inspect.isawaitable(close_result):
                    await close_result
                self._initialized = False
            except Exception as e:
                logging.error(f"Graphiti close error: {e}")
        
    async def add_episode(self, content: str, source_id: str = "user"):
        """
        Add a temporal episode to the graph.
        """
        if not self._initialized:
            logging.info("Graphiti: Auto-initializing before add_episode...")
            await self.initialize()
            
        if not self._initialized:
            logging.warning("Graphiti: Client not initialized. add_episode skipped.")
            return
            
        if self.client:
            try:
                from datetime import datetime, timezone
                logging.info(f"Graphiti: Adding episode for source '{source_id}' - Content: {content[:30]}...")
                
                # Determine source type
                source_type = EpisodeType.text if EpisodeType else "text"
                
                result = self.client.add_episode(
                    name=source_id, 
                    episode_body=content,
                    source=source_type,
                    source_description="Aether Memory Ingestion",
                    reference_time=datetime.now(timezone.utc)
                )
                
                # Await if the result is awaitable
                if inspect.isawaitable(result):
                    await result
                    
            except Exception as e:
                logging.error(f"Graphiti add_episode error: {e}")
        else:
            logging.debug("Graphiti: Skipping add_episode (Client not initialized)")

    async def add_triplet(self, source_node: Any, edge: Any, target_node: Any):
        """
        Add a manual fact triple.
        """
        if not self._initialized:
            logging.info("Graphiti: Auto-initializing before add_triplet...")
            await self.initialize()
            
        if not self._initialized:
            logging.warning("Graphiti: Client not initialized. add_triplet skipped.")
            return
            
        if self.client:
            try:
                logging.info(f"Graphiti: Adding manual triple: {source_node.name} -> {edge.fact} -> {target_node.name}")
                result = self.client.add_triplet(source_node, edge, target_node)
                
                # Await if the result is awaitable
                if inspect.isawaitable(result):
                    await result
                    
            except Exception as e:
                logging.error(f"Graphiti add_triplet error: {e}")
        else:
            logging.debug("Graphiti: Skipping add_triplet (Client not initialized)")

    async def search(self, query: str) -> List[Dict[str, Any]]:
        """
        Perform a hybrid search on the graph.
        """
        if not self._initialized:
            logging.info("Graphiti: Auto-initializing before search...")
            await self.initialize()
            
        if not self._initialized:
            logging.warning("Graphiti: Client not initialized. search returned empty list.")
            return []
            
        if self.client:
            try:
                logging.info(f"Graphiti: Performing hybrid search for '{query}'")
                res = self.client.search(query)
                if inspect.isawaitable(res):
                    results = await res
                else:
                    results = res
                
                logging.info(f"Graphiti search returned {len(results or [])} results.")
                return results or []
            except Exception as e:
                logging.error(f"Graphiti search error: {e}")
                return []
        else:
            logging.debug("Graphiti: Skipping search (Client not initialized)")
        return []


# Usage example:
"""
# Create the adapter
adapter = GraphitiAdapter(uri="neo4j://localhost:7687", user="neo4j", password="password")

# Connect and initialize (must be called in async context)
await adapter.connect()
await adapter.initialize()

# Use the adapter
await adapter.add_episode("Some content", "user_123")
results = await adapter.search("query text")

# Clean up
await adapter.close()
"""