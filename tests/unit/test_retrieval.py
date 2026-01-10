import pytest
from aether_sdk.resources.retrieval import RetrievalResource

@pytest.mark.asyncio
async def test_retrieval_hybrid(mock_vector_store, mock_llm):
    retrieval = RetrievalResource(
        project_id="test",
        vector_store=mock_vector_store,
        embedding_adapter=mock_llm
    )
    
    # Mock data in store
    mock_vector_store.upsert("id_1", [0.1]*384, {"content": "Target", "project_id": "test"})
    
    result = await retrieval.retrieve_hybrid("query")
    
    assert "chunks" in result
    assert "graph_context" in result
    
    # In hybrid search (mocked), we expect some results if vector store returns them
    # MockVectorStore returns 1 item always
    assert len(result["chunks"]) == 1
    assert result["chunks"][0] == "Target"
