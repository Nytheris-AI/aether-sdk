import pytest
from typing import List, Dict, Any, Tuple
from aether_sdk.core.protocols import LLMAdapterProtocol, VectorStoreProtocol, GraphStoreProtocol, EmbeddingAdapterProtocol

class MockLLMAdapter(LLMAdapterProtocol, EmbeddingAdapterProtocol):
    def generate_completion(self, messages: List[Dict[str, str]], **kwargs: Any) -> str:
        if kwargs.get("response_format", {}).get("type") == "json_object":
            return '{"entities": [], "relations": []}'
        return "Mocked Answer"

    def embed_text(self, text: str) -> List[float]:
        return [0.1] * 384

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        return [[0.1] * 384 for _ in range(len(texts))]

class MockVectorStore(VectorStoreProtocol):
    def __init__(self):
        self.points = {}

    def upsert(self, id: str, vector: List[float], metadata: Dict[str, Any]) -> None:
        self.points[id] = {"vector": vector, "metadata": metadata}

    def search(self, query_vector: List[float], top_k: int = 5) -> List[Tuple[str, float, Dict[str, Any]]]:
        # Return whatever is in the store (naive mock)
        results = []
        for pid, data in self.points.items():
            results.append((pid, 0.9, data["metadata"]))
        return results

class MockGraphStore(GraphStoreProtocol):
    def __init__(self):
        self.queries = []
        
    def execute_query(self, query: str, params: Dict[str, Any] = None) -> Any:
        self.queries.append((query, params))
        return []

@pytest.fixture
def mock_llm():
    return MockLLMAdapter()

@pytest.fixture
def mock_vector_store():
    return MockVectorStore()

@pytest.fixture
def mock_graph_store():
    return MockGraphStore()
