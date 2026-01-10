from typing import List, Dict, Any
import os
from openai import OpenAI, AsyncOpenAI
from aether_sdk.core.protocols import LLMAdapterProtocol, EmbeddingAdapterProtocol

class OpenAIAdapter:
    """
    Reference implementation for OpenAI.
    Implements both LLMAdapterProtocol and EmbeddingAdapterProtocol.
    """
    def __init__(self, api_key: str = None, model: str = "gpt-4o"):
        self.api_key = api_key or os.getenv("OPENAI_API_KEY")
        self.client = OpenAI(api_key=self.api_key)
        self.model = model
        self.embedding_model = "text-embedding-3-small"

    def generate_completion(self, messages: List[Dict[str, str]], **kwargs: Any) -> str:
        """Synchronous completion."""
        response = self.client.chat.completions.create(
            model=kwargs.get("model", self.model),
            messages=messages,
            **{k:v for k,v in kwargs.items() if k != "model"}
        )
        return response.choices[0].message.content

    def embed_text(self, text: str) -> List[float]:
        """Embed single text."""
        text = text.replace("\n", " ")
        return self.client.embeddings.create(input=[text], model=self.embedding_model).data[0].embedding

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        """Embed batch."""
        clean_texts = [t.replace("\n", " ") for t in texts]
        data = self.client.embeddings.create(input=clean_texts, model=self.embedding_model).data
        return [d.embedding for d in data]
