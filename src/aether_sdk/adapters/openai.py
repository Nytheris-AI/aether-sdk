from typing import List, Dict, Any
import os
from openai import OpenAI, AsyncOpenAI
from aether_sdk.core.protocols import LLMAdapterProtocol, EmbeddingAdapterProtocol

class OpenAIAdapter:
    """
    Reference implementation for OpenAI.
    Implements both LLMAdapterProtocol and EmbeddingAdapterProtocol.
    """
    def __init__(self, api_key: str = None, base_url: str = None, model: str = "gpt-4o"):
        self.api_key = api_key or os.getenv("OPENAI_API_KEY")
        self.base_url = base_url or os.getenv("OPENAI_BASE_URL")
        self.client = OpenAI(api_key=self.api_key, base_url=self.base_url)
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

    def generate_structured(
        self,
        messages: List[Dict[str, str]],
        response_schema: Dict[str, Any],
        **kwargs: Any,
    ) -> Dict[str, Any]:
        """Generate structured JSON output using OpenAI JSON mode."""
        import json
        
        # Ensure we ask for JSON
        kwargs["response_format"] = {"type": "json_object"}
        
        # If schema is provided, we might want to append it to system prompt or use structured outputs if supported.
        # For compatibility with standard JSON mode, we append instructions to the prompt if not already likely present.
        # But per the protocol, the user passes a schema.
        # Simple implementation: Let's assume the user put instructions in messages, 
        # OR we can try to force tools/functions in future.
        # For now, strict JSON mode:
        
        response = self.client.chat.completions.create(
            model=kwargs.get("model", self.model),
            messages=messages,
            **{k:v for k,v in kwargs.items() if k != "model"}
        )
        content = response.choices[0].message.content
        return json.loads(content)

    def embed_text(self, text: str) -> List[float]:
        """Embed single text."""
        text = text.replace("\n", " ")
        return self.client.embeddings.create(input=[text], model=self.embedding_model).data[0].embedding

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        """Embed batch."""
        clean_texts = [t.replace("\n", " ") for t in texts]
        data = self.client.embeddings.create(input=clean_texts, model=self.embedding_model).data
        return [d.embedding for d in data]
