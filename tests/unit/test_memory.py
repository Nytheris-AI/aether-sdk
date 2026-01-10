import pytest
from aether_sdk.memory import MemoryManager

@pytest.mark.asyncio
async def test_memory_manager_init(mock_llm, mock_vector_store, mock_graph_store):
    manager = MemoryManager(
        project_id="test_project",
        llm_adapter=mock_llm,
        embedding_adapter=mock_llm,
        vector_store=mock_vector_store,
        graph_store=mock_graph_store
    )
    
    assert manager.project_id == "test_project"
    assert manager.ingestion is not None
    assert manager.retrieval is not None
    assert manager.brain is not None

@pytest.mark.asyncio
async def test_memory_ingest_delegation(mock_llm, mock_vector_store):
    manager = MemoryManager(
        project_id="test",
        llm_adapter=mock_llm,
        embedding_adapter=mock_llm,
        vector_store=mock_vector_store
    )
    
    # Simple smoke test that it runs without error (mock upserts happen)
    await manager.ingest_text("Hello World", "test.txt")
    
    # Verify mock vector store received data
    assert len(mock_vector_store.points) > 0

@pytest.mark.asyncio
async def test_memory_ask_delegation(mock_llm, mock_vector_store):
    manager = MemoryManager(
        project_id="test",
        llm_adapter=mock_llm,
        embedding_adapter=mock_llm,
        vector_store=mock_vector_store
    )
    
    answer = await manager.ask("What is hello?")
    assert answer == "Mocked Answer"
