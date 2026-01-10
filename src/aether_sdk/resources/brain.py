from typing import List, Dict, Any
from aether_sdk.resources.retrieval import RetrievalResource
from aether_sdk.resources.distillation import DistillationResource

class BrainResource:
    """
    Handles Intelligence Orchestration (Answering).
    Orchestrates Retrieval -> Distillation.
    """
    def __init__(
        self,
        retrieval_resource: RetrievalResource,
        distillation_resource: DistillationResource,
    ):
        self._retrieval = retrieval_resource
        self._distillation = distillation_resource

    async def generate_answer(self, query: str) -> str:
        """
        Ask a question to the brain.
        """
        # 1. Retrieve Context
        context_data = await self._retrieval.retrieve_hybrid(query)
        chunks = context_data.get("chunks", [])
        graph_data = context_data.get("graph_context", [])
        
        # 2. Distill Answer
        return await self._distillation.distill(query, chunks, graph_data)
