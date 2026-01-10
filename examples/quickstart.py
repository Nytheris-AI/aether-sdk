import asyncio
import os
import sys
from typing import List, Dict, Any

# Ensure we can import the SDK (for local development)
sys.path.append(os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "src")))

from aether_sdk.memory import MemoryManager
from aether_sdk.adapters.qdrant import QdrantAdapter
# from aether_sdk.adapters.openai import OpenAIAdapter # Commented out for mock verification

from aether_sdk.core.protocols import GraphStoreProtocol

class MockLLMAdapter:
    """Mock Adapter for verifying SDK architecture."""
    def generate_completion(self, messages: List[Dict[str, str]], **kwargs: Any) -> str:
        # Check if we are extracting (JSON)
        if kwargs.get("response_format") == {"type": "json_object"}:
            return """{
                "entities": [{"name": "Aether SDK", "type": "Code", "description": "The SDK"}],
                "relations": []
            }"""
        return "This is a mocked answer demonstrating the Protocol Architecture."

    def embed_text(self, text: str) -> List[float]:
        return [0.1] * 384

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        return [[0.1] * 384 for _ in texts]

class MockGraphAdapter:
    """Mock Graph Store."""
    def upsert(self, whatever): pass
    # Add protocol methods as needed (placeholder)

async def main():
    # 1. User sets up their infrastructure
    # In a real app, these credentials come from env vars
    qdrant_url = os.getenv("QDRANT_URL", ":memory:") # Use memory for demo if set
    
    print("--- 1. Initialize Adapters ---")
    llm_adapter = MockLLMAdapter() # Using Mock instead of Real API
    vector_adapter = QdrantAdapter(url=qdrant_url, collection_name="demo_memory")
    graph_adapter = MockGraphAdapter()
    
    print(f"LLM: {llm_adapter.__class__.__name__}")
    print(f"Vector: {vector_adapter.__class__.__name__}")
    print(f"Graph: {graph_adapter.__class__.__name__}")

    # 2. User initializes our SDK (MemoryManager)
    print("\n--- 2. Initialize MemoryManager ---")
    memory = MemoryManager(
        project_id="demo-project-v3",
        llm_adapter=llm_adapter,
        embedding_adapter=llm_adapter, # OpenAI does both
        vector_store=vector_adapter,
        graph_store=graph_adapter
    )
    
    # 3. Ingest Data
    print("\n--- 3. Ingesting Data ---")
    text = """
    The Aether Protocol-Based SDK allows users to bring their own infrastructure.
    It separates Memory Logic (Aether) from Execution Logic (Adapters).
    This ensures zero vendor lock-in and maximum flexibility.
    """
    await memory.ingest_text(text, filename="architecture.txt")
    print("Ingestion complete.")

    # 4. Ask Question
    print("\n--- 4. Asking Question ---")
    query = "What is the main benefit of the Protocol-Based SDK?"
    print(f"Query: {query}")
    answer = await memory.ask(query)
    
    print(f"\nAnswer: {answer}")

if __name__ == "__main__":
    asyncio.run(main())
