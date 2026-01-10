from typing import Optional, List, Dict, Any
from aether_sdk.core.protocols import LLMAdapterProtocol, EmbeddingAdapterProtocol, VectorStoreProtocol, GraphStoreProtocol
from aether_sdk.types.shared import Document, ContextResult
from aether_sdk.resources.ingestion import IngestionResource
from aether_sdk.resources.retrieval import RetrievalResource
from aether_sdk.resources.brain import BrainResource
from aether_sdk.resources.distillation import DistillationResource
from aether_sdk.resources.extraction import ExtractionResource
from aether_sdk.resources.parsing import ParsingResource

class MemoryManager:
    # ... (init args same)
    def __init__(
        self,
        project_id: str,
        llm_adapter: LLMAdapterProtocol,
        embedding_adapter: EmbeddingAdapterProtocol,
        vector_store: VectorStoreProtocol,
        graph_store: Optional[GraphStoreProtocol] = None,
    ):
        self.project_id = project_id
        
        # 1. Store Adapters
        self._llm = llm_adapter
        self._embeddings = embedding_adapter
        self._vectors = vector_store
        self._graph = graph_store
        
        # 2. Initialize Resources (The Logic Layer)
        
        self.parsing = ParsingResource()
        
        self.extraction = ExtractionResource(
            llm_adapter=self._llm
        )
        
        self.ingestion = IngestionResource(
            project_id=project_id,
            vector_store=self._vectors,
            graph_store=self._graph,
            embedding_adapter=self._embeddings,
            extraction_resource=self.extraction,
            parsing_resource=self.parsing
        )
        
        self.retrieval = RetrievalResource(
            project_id=project_id,
            vector_store=self._vectors,
            graph_store=self._graph,
            embedding_adapter=self._embeddings
        )
        
        self.distillation = DistillationResource(
            llm_adapter=self._llm
        )
        
        self.brain = BrainResource(
            retrieval_resource=self.retrieval,
            distillation_resource=self.distillation
        )

    def create_conversation(self, user_id: str) -> "Conversation":
        """Creates a stateful conversation handler."""
        # Placeholder for future multi-turn logic
        pass

    async def ingest_text(self, content: str, filename: str = "text.txt") -> None:
        """Facade method for ingestion."""
        doc = Document(
            project_id=self.project_id,
            filename=filename,
            content=content,
            content_type="text/plain"
        )
        await self.ingestion.ingest_document(doc)

    async def ask(self, query: str) -> str:
        """Facade method for Q&A."""
        return await self.brain.generate_answer(query)
