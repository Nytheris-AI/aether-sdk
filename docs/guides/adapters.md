# Built-in Adapters

Adapters are concrete implementations of Aether protocols for specific providers.

## LLM Adapters

### LiteLLMAdapter (Recommended)
Uses the `litellm` library to support 100+ providers via a unified interface.
- **Supports**: OpenAI, Anthropic, Azure, OpenRouter, Google Vertex, Ollama, and more.
- **Key Features**: Auto-dropping incompatible params, OpenRouter headers support.

```python
adapter = LiteLLMAdapter(
    model="openrouter/anthropic/claude-3-opus",
    api_key="sk-or-...",
    site_url="https://myapp.com", # OpenRouter specific
    app_name="My App"             # OpenRouter specific
)
```

### OpenAIAdapter
Pure OpenAI implementation for those who prefer native SDK usage.

## Vector Store Adapters

### QdrantAdapter
- High-performance, production-ready vector database.
- Supports both cloud and local instances.

### ChromaAdapter
- Lightweight, developer-friendly vector database.
- Ideal for local development and prototyping.

## Graph Store Adapters

### Neo4jAdapter
- Standard for property graph databases.
- Used for high-density relationship mapping and complex RAG.

---

## Creating a Custom Adapter

To create a custom adapter, simply implement the relevant protocol:

```python
from aether_sdk.core.protocols import LLMAdapterProtocol

class MyCustomLLM(LLMAdapterProtocol):
    def generate_completion(self, messages, **kwargs):
        # Your custom logic here
        return "Custom Hello!"
```
