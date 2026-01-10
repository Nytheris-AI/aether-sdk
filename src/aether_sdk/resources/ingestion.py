from typing import List, Optional
from aether_sdk.core.protocols import LLMAdapterProtocol, EmbeddingAdapterProtocol, VectorStoreProtocol, GraphStoreProtocol
from aether_sdk.types.shared import Document, KnowledgeGraph
from aether_sdk.resources.extraction import ExtractionResource
from aether_sdk.resources.parsing import ParsingResource

class IngestionResource:
    """
    Handles data ingestion pipeline: Parse -> Chunk -> Extract -> Store.
    """
    def __init__(
        self,
        project_id: str,
        vector_store: VectorStoreProtocol,
        embedding_adapter: EmbeddingAdapterProtocol,
        extraction_resource: Optional[ExtractionResource] = None,
        parsing_resource: Optional[ParsingResource] = None,
        graph_store: Optional[GraphStoreProtocol] = None,
        initial_chunk_size: int = 1000,
    ):
        self.project_id = project_id
        self._vectors = vector_store
        self._graph = graph_store
        self._embeddings = embedding_adapter
        self._extractor = extraction_resource
        self._parser = parsing_resource
        self.chunk_size = initial_chunk_size

    def _naive_chunk(self, text: str) -> List[str]:
        """Simple chunking strategy (Placeholder for advanced logic)."""
        return [text[i:i+self.chunk_size] for i in range(0, len(text), self.chunk_size)]

    async def ingest_document(self, doc: Document) -> None:
        """
        Main ingestion flow.
        1. Parse (if needed)
        2. Chunk
        3. Embed & Vector Store
        4. Extract & Graph Store
        """
        # 1. Parse (Wait, doc.content is usually text, but if we support raw ingestion later...)
        # For now assume doc.content is string text. 
        # Future: self._parser.parse_bytes(doc.raw_content, doc.content_type)
        
        chunks = self._naive_chunk(doc.content)
        
        # 2. Embeddings
        vectors = self._embeddings.embed_batch(chunks)
        
        import uuid
        
        # 3. Vector Storage
        for i, (chunk, vector) in enumerate(zip(chunks, vectors)):
            # Generate deterministic UUID for the chunk
            chunk_str_id = f"{doc.id}_chunk_{i}"
            chunk_id = str(uuid.uuid5(uuid.NAMESPACE_DNS, chunk_str_id))
            
            metadata = {
                "project_id": self.project_id,
                "filename": doc.filename,
                "content": chunk,
                "chunk_index": i,
                "original_id": chunk_str_id 
            }
            self._vectors.upsert(id=chunk_id, vector=vector, metadata=metadata)
            
            # 4. Knowledge Graph Extraction (Per chunk)
            if self._extractor and self._graph:
                # Extract graph from this chunk
                kg = await self._extractor.extract(chunk)
                # Store in Graph DB (TODO: VectorStoreProtocol doesn't store graph, we need GraphStoreProtocol)
                # For now just log or Placeholder pass until GraphStoreProtocol is fully active
                pass
            
        doc.status = "completed"
