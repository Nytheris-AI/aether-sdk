from enum import Enum
from typing import Dict, Any, List
from aether_sdk.core.protocols import LLMAdapterProtocol

class QueryComplexity(Enum):
    SIMPLE = "simple"   # Direct lookup
    COMPLEX = "complex" # Multi-hop reasoning

class QueryPlanner:
    """
    Estimates the complexity of a user query and generates a retrieval plan.
    Adapted from SimpleMem.HybridRetriever.
    """
    
    def __init__(self, llm_client: LLMAdapterProtocol = None):
        self.llm_client = llm_client

    async def estimate_complexity(self, query: str) -> QueryComplexity:
        """Determine if query needs simple vector search or graph traversal."""
        # Heuristic for MVP: length + keywords like 'how', 'why', 'connection'
        if len(query.split()) > 10 or any(w in query.lower() for w in ["how", "why", "relationship", "connect"]):
            return QueryComplexity.COMPLEX
        return QueryComplexity.SIMPLE

    async def generate_plan(self, query: str) -> Dict[str, Any]:
        """
        Generates a query plan.
        """
        # MVP: Mock plan
        return {
            "query": query,
            "steps": ["vector_search", "graph_traversal"] if await self.estimate_complexity(query) == QueryComplexity.COMPLEX else ["vector_search"]
        }

