import pytest
import uuid
from aether_sdk.resources.ingestion import IngestionResource
from aether_sdk.types.shared import Document

@pytest.mark.asyncio
async def test_ingestion_flow(mock_vector_store, mock_llm):
    ingestion = IngestionResource(
        project_id="test_proj",
        vector_store=mock_vector_store,
        embedding_adapter=mock_llm,
        initial_chunk_size=10 # Small size to force chunks
    )
    
    doc = Document(
        project_id="test_proj",
        filename="small.txt",
        content="This is a test document with enough length.",
        content_type="text/plain"
    )
    
    await ingestion.ingest_document(doc)
    
    # Verify chunks
    points = mock_vector_store.points
    assert len(points) > 1 # Should be chunked
    
    # Check Metadata
    first_key = list(points.keys())[0]
    payload = points[first_key]["metadata"]
    assert payload["project_id"] == "test_proj"
    assert payload["filename"] == "small.txt"
    assert "chunk_index" in payload

    # Check ID is UUID
    try:
        uuid.UUID(first_key)
    except ValueError:
        pytest.fail("Chunk ID is not a valid UUID")
