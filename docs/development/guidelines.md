# Context Engineering SDK - Code Style & Architecture Guidelines

**Version:** 1.0  
**Last Updated:** January 2026  
**Status:** Living Document

---

## Integration Strategy

### Protocol-Based Adapters (Not Wrappers!)

```python
from typing import Protocol, Any
from abc import abstractmethod

# ============================================
# OUR CODE: Define what we need, not how
# ============================================

class LLMAdapterProtocol(Protocol):
    """Protocol for LLM integrations.
    
    Users implement this for their chosen LLM provider.
    We don't care HOW they call the LLM, just that they can.
    """
    
    @abstractmethod
    def generate_completion(
        self,
        messages: list[dict[str, str]],
        **kwargs: Any,
    ) -> str:
        """Generate completion from messages.
        
        Implementation is user's responsibility.
        They handle: API calls, retries, rate limits, costs.
        We handle: Context assembly, memory retrieval.
        """
        ...


class EmbeddingAdapterProtocol(Protocol):
    """Protocol for embedding generation.
    
    Users choose their embedding provider.
    """
    
    @abstractmethod
    def embed_text(self, text: str) -> list[float]:
        """Generate embedding vector for text."""
        ...
    
    @abstractmethod
    def embed_batch(self, texts: list[str]) -> list[list[float]]:
        """Generate embeddings for multiple texts."""
        ...


class VectorStoreProtocol(Protocol):
    """Protocol for vector storage.
    
    Users bring their own vector database.
    """
    
    @abstractmethod
    def upsert(self, id: str, vector: list[float], metadata: dict) -> None:
        """Store vector with metadata."""
        ...
    
    @abstractmethod
    def search(
        self,
        query_vector: list[float],
        top_k: int = 5,
    ) -> list[tuple[str, float, dict]]:
        """Search for similar vectors."""
        ...


# ============================================
# USER'S CODE: They implement for their stack
# ============================================

# Example: User's OpenAI adapter
from openai import OpenAI

class UserOpenAIAdapter:
    """User implements this - NOT US."""
    
    def __init__(self, client: OpenAI):
        self.client = client
    
    def generate_completion(self, messages, **kwargs):
        response = self.client.chat.completions.create(
            model=kwargs.get("model", "gpt-4"),
            messages=messages,
            temperature=kwargs.get("temperature", 0.7),
        )
        return response.choices[0].message.content
    
    def embed_text(self, text: str) -> list[float]:
        response = self.client.embeddings.create(
            model="text-embedding-3-small",
            input=text,
        )
        return response.data[0].embedding
    
    def embed_batch(self, texts: list[str]) -> list[list[float]]:
        response = self.client.embeddings.create(
            model="text-embedding-3-small",
            input=texts,
        )
        return [item.embedding for item in response.data]


# Example: User's Pinecone adapter
import pinecone

class UserPineconeAdapter:
    """User implements this - NOT US."""
    
    def __init__(self, index_name: str):
        self.index = pinecone.Index(index_name)
    
    def upsert(self, id: str, vector: list[float], metadata: dict) -> None:
        self.index.upsert([(id, vector, metadata)])
    
    def search(self, query_vector, top_k=5):
        results = self.index.query(
            vector=query_vector,
            top_k=top_k,
            include_metadata=True,
        )
        return [
            (match.id, match.score, match.metadata)
            for match in results.matches
        ]


# ============================================
# OUR CODE: Memory management using adapters
# ============================================

class MemoryManager:
    """Our core business: memory and context management."""
    
    def __init__(
        self,
        llm_adapter: LLMAdapterProtocol,
        embedding_adapter: EmbeddingAdapterProtocol,
        vector_store: VectorStoreProtocol,
        storage: str = "memory://",  # Redis, Postgres, etc.
    ):
        """Initialize with user-provided adapters.
        
        We don't care WHAT they use, just that it implements our protocol.
        """
        self._llm = llm_adapter
        self._embeddings = embedding_adapter
        self._vectors = vector_store
        self._storage = self._init_storage(storage)
    
    def create_conversation(self, user_id: str) -> "Conversation":
        """Create conversation with memory."""
        return Conversation(
            user_id=user_id,
            memory_manager=self,
        )
```

### Integration Examples

```python
# ============================================
# USER SETUP: Minimal, clean, explicit
# ============================================

from openai import OpenAI
from anthropic import Anthropic
import pinecone
from context_sdk import MemoryManager

# User's infrastructure
openai_client = OpenAI(api_key="sk-...")
pinecone.init(api_key="...", environment="...")

# User's adapters (they own this code)
llm_adapter = UserOpenAIAdapter(openai_client)
vector_adapter = UserPineconeAdapter("my-index")

# Our SDK: Memory management
memory = MemoryManager(
    llm_adapter=llm_adapter,
    embedding_adapter=llm_adapter,  # Same adapter for both
    vector_store=vector_adapter,
    storage="redis://localhost:6379/0",
)

# Pure memory operations (our value)
conversation = memory.create_conversation("user-123")
conversation.add_message("user", "Remember my favorite color is blue")
conversation.add_message("assistant", "I'll remember that!")

# Later...
conversation.add_message("user", "What's my favorite color?")

# We retrieve relevant context from memory
context = conversation.get_relevant_context(query="favorite color")
# context = ["Remember my favorite color is blue", "I'll remember that!"]

# We assemble the prompt with context
messages = conversation.build_messages_with_context(
    current_message="What's my favorite color?",
    max_context_tokens=1000,
)

# User generates response (using THEIR LLM client)
response = llm_adapter.generate_completion(messages)
conversation.add_message("assistant", response)


# ============================================
# SWITCHING PROVIDERS: Easy!
# ============================================

# User wants to switch to Anthropic? No problem!
anthropic_client = Anthropic(api_key="sk-ant-...")

class UserAnthropicAdapter:
    def __init__(self, client: Anthropic):
        self.client = client
    
    def generate_completion(self, messages, **kwargs):
        response = self.client.messages.create(
            model=kwargs.get("model", "claude-3-5-sonnet-20241022"),
            messages=messages,
            max_tokens=kwargs.get("max_tokens", 1024),
        )
        return response.content[0].text

# Just swap the adapter!
memory = MemoryManager(
    llm_adapter=UserAnthropicAdapter(anthropic_client),
    embedding_adapter=llm_adapter,  # Can keep OpenAI for embeddings
    vector_store=vector_adapter,
    storage="redis://localhost:6379/0",
)

# Everything else stays the same - our memory logic doesn't change
```

### Why This Architecture?

**Benefits:**
1. **Lean codebase** - We don't maintain LLM API clients
2. **Zero vendor lock-in** - Users control their infrastructure
3. **Always up-to-date** - Users use official SDKs directly
4. **Clear responsibility** - We do memory, they do LLM calls
5. **Maximum flexibility** - Users can use ANY LLM, ANY vector DB
6. **Production-ready** - Users handle auth, retries, rate limits their way

**Trade-offs:**
- Users write simple adapters (typically 20-50 lines)
- We provide adapter templates and examples
- Slight setup overhead, but massive long-term benefits

---

## 🎯 Our Core Business

**We build ONE thing exceptionally well: AI Memory Layer and Context Engineering**

### What We DO Build
✅ Memory management and persistence  
✅ Context window optimization  
✅ Semantic search and retrieval  
✅ Memory compression and summarization  
✅ Multi-turn conversation state  
✅ Context-aware prompt engineering  

### What We DON'T Build
❌ LLM API wrappers (use OpenAI SDK, Anthropic SDK directly)  
❌ Generic HTTP clients (use httpx, requests)  
❌ Vector database implementations (integrate with Pinecone, Weaviate, etc.)  
❌ Embedding models (use OpenAI, Cohere, HuggingFace)  

**Philosophy: We are a specialized tool that works WITH existing SDKs, not replacing them.**

---

## Table of Contents

1. [Core Principles](#core-principles)
2. [Architecture Philosophy](#architecture-philosophy)
3. [Integration Strategy](#integration-strategy)
4. [Code Style Guidelines](#code-style-guidelines)
5. [Dependency Injection Pattern](#dependency-injection-pattern)
6. [Modularity & Plugin System](#modularity-plugin-system)
7. [API Design Principles](#api-design-principles)
8. [Error Handling](#consistent-error-handling)

---

## Core Principles

### 🎯 The Five Pillars

1. **Focused on Core Value**
   - Build ONLY memory and context engineering features
   - Integrate with, don't replace, existing SDKs
   - Lean codebase, zero bloat
   - Every line of code serves our core mission

2. **Developer-Friendly Integration**
   - Works seamlessly with OpenAI, Anthropic, etc.
   - Minimal setup, maximum value
   - Sensible defaults for memory management
   - Clear separation between our code and integrations

3. **Modular Memory Components**
   - Pluggable memory stores (in-memory, Redis, Postgres)
   - Swappable retrieval strategies (semantic, recency, importance)
   - Customizable compression algorithms
   - Independent context window optimization

4. **Protocol-Based Adapters**
   - Protocols for external integrations (LLMs, vector DBs, embeddings)
   - Users provide their own clients
   - Zero vendor lock-in
   - Maximum flexibility

5. **Production-Grade Memory**
   - Persistent, fault-tolerant storage
   - Efficient memory retrieval (`<100ms`)
   - Scalable to millions of interactions
   - Backward compatibility guarantees

---

## Architecture Philosophy

### Context Engineering Layered Architecture

```
┌─────────────────────────────────────────────────────┐
│         Public API Layer                            │
│  (MemoryManager, ContextBuilder, ConversationState) │ ← What developers use
├─────────────────────────────────────────────────────┤
│         Memory Core Layer                           │
│  (Storage, Retrieval, Compression, Summarization)   │ ← Our core business
├─────────────────────────────────────────────────────┤
│         Integration Adapters (Protocols)            │
│  (LLMAdapter, VectorDBAdapter, EmbeddingAdapter)    │ ← User provides implementations
├─────────────────────────────────────────────────────┤
│         External SDKs (NOT OUR CODE)                │
│  (OpenAI SDK, Anthropic SDK, Pinecone, etc.)        │ ← User's dependencies
└─────────────────────────────────────────────────────┘
```

### Component Interaction Model

```python
# USER'S CODE: They own the LLM client
from openai import OpenAI
from anthropic import Anthropic

openai_client = OpenAI(api_key="...")
anthropic_client = Anthropic(api_key="...")

# OUR CODE: We manage memory and context
from context_sdk import MemoryManager, LLMAdapter

# User creates adapter (simple wrapper they control)
class OpenAIAdapter(LLMAdapter):
    def __init__(self, client: OpenAI):
        self.client = client
    
    def generate(self, messages):
        return self.client.chat.completions.create(
            model="gpt-4",
            messages=messages
        )

# Our SDK manages memory, not LLM calls
memory = MemoryManager(
    storage="redis://localhost",
    llm_adapter=OpenAIAdapter(openai_client)  # User's adapter
)

# We enhance their workflow with memory
conversation = memory.create_conversation("user-123")
conversation.add_message("user", "What's my name?")
response = conversation.generate_reply()  # Uses user's LLM via adapter
```

### Clean Separation of Concerns

```
OUR RESPONSIBILITIES:
├── Memory persistence and retrieval
├── Context window optimization
├── Conversation state management
├── Semantic search over history
├── Memory compression
└── Prompt context assembly

USER'S RESPONSIBILITIES:
├── LLM API calls (OpenAI, Anthropic, etc.)
├── Vector database operations (optional)
├── Embedding generation (optional)
├── Rate limiting and retries
└── Authentication and billing
```

---

## Code Style Guidelines

### 1. Naming Conventions

```python
# Classes: PascalCase (clear, descriptive nouns)
class CompletionResource:
    pass

class HTTPClientProtocol:
    pass

# Functions/Methods: snake_case (verb phrases)
def create_completion() -> Completion:
    pass

def validate_api_key(key: str) -> bool:
    pass

# Constants: SCREAMING_SNAKE_CASE
DEFAULT_TIMEOUT = 30.0
MAX_RETRIES = 3
API_VERSION = "v1"

# Private members: _leading_underscore
def _internal_helper() -> None:
    pass

# Type variables: Short, descriptive
T = TypeVar('T')
ResponseT = TypeVar('ResponseT', bound='BaseResponse')
ConfigT = TypeVar('ConfigT', bound='BaseConfig')
```

### 2. Import Organization

```python
"""Module docstring explaining purpose and usage."""

# Standard library imports
from __future__ import annotations
import os
import sys
from typing import Any, Optional, Protocol, TypeVar

# Third-party imports
import httpx
from pydantic import BaseModel, Field

# Local application imports
from ai_sdk.core.protocols import HTTPClientProtocol
from ai_sdk.core.retry import RetryPolicy
from ai_sdk.exceptions import APIError, ValidationError
from ai_sdk.types import CompletionRequest, CompletionResponse

# Type checking imports (avoid circular dependencies)
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from ai_sdk.client import AIClient
```

### 3. Code Formatting Standards

**Line Length:** 100 characters (enforced by Ruff)

```python
# ✅ Good: Readable, within limits
def create_completion(
    prompt: str,
    model: str = "gpt-4",
    temperature: float = 0.7,
) -> CompletionResponse:
    pass

# ❌ Bad: Too long
def create_completion(prompt: str, model: str = "gpt-4", temperature: float = 0.7, max_tokens: int = 1000) -> CompletionResponse:
    pass
```

**Whitespace:**

```python
# ✅ Good: Clear separation of concerns
class CompletionResource:
    """Handles completion operations."""
    
    def __init__(self, client: HTTPClientProtocol) -> None:
        self._client = client
    
    def create(self, request: CompletionRequest) -> CompletionResponse:
        """Create a completion."""
        validated = self._validate(request)
        return self._client.post("/completions", data=validated)
    
    def _validate(self, request: CompletionRequest) -> dict[str, Any]:
        """Validate and transform request."""
        return request.model_dump(exclude_none=True)


# ❌ Bad: Cramped, hard to read
class CompletionResource:
    def __init__(self, client: HTTPClientProtocol) -> None:
        self._client = client
    def create(self, request: CompletionRequest) -> CompletionResponse:
        validated = self._validate(request)
        return self._client.post("/completions", data=validated)
    def _validate(self, request: CompletionRequest) -> dict[str, Any]:
        return request.model_dump(exclude_none=True)
```

### 4. Docstring Standards (Google Style)

```python
def create_completion(
    prompt: str,
    model: str = "gpt-4",
    temperature: float = 0.7,
    max_tokens: Optional[int] = None,
) -> CompletionResponse:
    """Create a text completion using the specified model.
    
    This method sends a completion request to the AI API and returns
    the generated text response. It supports streaming and non-streaming
    modes based on the configuration.
    
    Args:
        prompt: The input text to generate a completion for.
        model: The model identifier to use for generation.
            Defaults to "gpt-4".
        temperature: Controls randomness in generation (0.0 to 2.0).
            Higher values produce more diverse outputs.
        max_tokens: Maximum number of tokens to generate.
            If None, uses the model's default limit.
    
    Returns:
        CompletionResponse containing the generated text and metadata.
    
    Raises:
        ValidationError: If the prompt is empty or parameters are invalid.
        APIError: If the API request fails or returns an error.
        RateLimitError: If the rate limit is exceeded.
    
    Example:
        >>> client = AIClient(api_key="sk-...")
        >>> response = client.completions.create(
        ...     prompt="Explain quantum computing",
        ...     temperature=0.5
        ... )
        >>> print(response.text)
        "Quantum computing is a type of computation..."
    
    Note:
        Temperature values above 1.5 may produce unpredictable results.
        For deterministic outputs, set temperature to 0.0.
    """
    pass
```

---

## Dependency Injection Pattern

### Protocol-Based Design

```python
from typing import Protocol, runtime_checkable

@runtime_checkable
class HTTPClientProtocol(Protocol):
    """Protocol defining HTTP client interface.
    
    This protocol allows any HTTP client implementation to be used
    as long as it implements these methods.
    """
    
    def request(
        self,
        method: str,
        url: str,
        **kwargs: Any,
    ) -> httpx.Response:
        """Send an HTTP request."""
        ...
    
    def close(self) -> None:
        """Close the HTTP client and cleanup resources."""
        ...


@runtime_checkable
class RetryPolicyProtocol(Protocol):
    """Protocol for retry logic implementations."""
    
    def should_retry(
        self,
        attempt: int,
        exception: Exception,
    ) -> bool:
        """Determine if a request should be retried."""
        ...
    
    def get_wait_time(self, attempt: int) -> float:
        """Calculate wait time before next retry."""
        ...
```

### Constructor Injection (Primary Method)

```python
class CompletionResource:
    """Resource for managing completions."""
    
    def __init__(
        self,
        http_client: HTTPClientProtocol,
        retry_policy: RetryPolicyProtocol,
        validator: Optional[ValidatorProtocol] = None,
    ) -> None:
        """Initialize completion resource.
        
        Args:
            http_client: HTTP client for making API requests.
            retry_policy: Policy for handling retries.
            validator: Optional custom validator for requests.
        """
        self._http_client = http_client
        self._retry_policy = retry_policy
        self._validator = validator or DefaultValidator()
    
    def create(self, request: CompletionRequest) -> CompletionResponse:
        """Create a completion with automatic retries."""
        validated = self._validator.validate(request)
        
        attempt = 0
        while True:
            try:
                return self._http_client.request(
                    "POST",
                    "/completions",
                    json=validated,
                )
            except Exception as e:
                if not self._retry_policy.should_retry(attempt, e):
                    raise
                
                wait_time = self._retry_policy.get_wait_time(attempt)
                time.sleep(wait_time)
                attempt += 1
```

### Factory Pattern for Complex Construction

```python
class ClientFactory:
    """Factory for creating configured AI clients."""
    
    @staticmethod
    def create_default(api_key: str) -> AIClient:
        """Create client with sensible defaults."""
        return ClientFactory.create(
            api_key=api_key,
            http_client=httpx.Client(),
            retry_policy=ExponentialBackoffRetry(),
        )
    
    @staticmethod
    def create(
        api_key: str,
        http_client: HTTPClientProtocol,
        retry_policy: RetryPolicyProtocol,
        middleware: Optional[list[MiddlewareProtocol]] = None,
    ) -> AIClient:
        """Create fully configured client."""
        authenticator = BearerAuthenticator(api_key)
        
        # Wrap HTTP client with authentication
        authenticated_client = AuthenticatedHTTPClient(
            client=http_client,
            authenticator=authenticator,
        )
        
        # Build middleware chain
        middleware_chain = MiddlewareChain(middleware or [])
        
        return AIClient(
            http_client=authenticated_client,
            retry_policy=retry_policy,
            middleware=middleware_chain,
        )
```

### Builder Pattern for Fluent APIs

```python
class CompletionRequestBuilder:
    """Builder for constructing completion requests declaratively."""
    
    def __init__(self) -> None:
        self._prompt: str = ""
        self._model: str = "gpt-4"
        self._temperature: float = 0.7
        self._max_tokens: Optional[int] = None
        self._stop_sequences: list[str] = []
    
    def with_prompt(self, prompt: str) -> CompletionRequestBuilder:
        """Set the completion prompt."""
        self._prompt = prompt
        return self
    
    def with_model(self, model: str) -> CompletionRequestBuilder:
        """Set the model to use."""
        self._model = model
        return self
    
    def with_temperature(self, temperature: float) -> CompletionRequestBuilder:
        """Set the temperature for randomness."""
        if not 0.0 <= temperature <= 2.0:
            raise ValueError("Temperature must be between 0.0 and 2.0")
        self._temperature = temperature
        return self
    
    def with_max_tokens(self, max_tokens: int) -> CompletionRequestBuilder:
        """Set maximum tokens to generate."""
        if max_tokens <= 0:
            raise ValueError("max_tokens must be positive")
        self._max_tokens = max_tokens
        return self
    
    def add_stop_sequence(self, sequence: str) -> CompletionRequestBuilder:
        """Add a stop sequence."""
        self._stop_sequences.append(sequence)
        return self
    
    def build(self) -> CompletionRequest:
        """Build the final request object."""
        if not self._prompt:
            raise ValueError("Prompt is required")
        
        return CompletionRequest(
            prompt=self._prompt,
            model=self._model,
            temperature=self._temperature,
            max_tokens=self._max_tokens,
            stop=self._stop_sequences or None,
        )


# Usage: Declarative and readable
request = (
    CompletionRequestBuilder()
    .with_prompt("Explain quantum computing")
    .with_model("gpt-4")
    .with_temperature(0.5)
    .with_max_tokens(500)
    .add_stop_sequence("\n\n")
    .build()
)
```

---

## Modularity & Plugin System

### Plugin Interface

```python
from abc import ABC, abstractmethod

class PluginProtocol(Protocol):
    """Base protocol for memory plugins."""
    
    @property
    def name(self) -> str:
        """Unique plugin identifier."""
        ...
    
    @property
    def version(self) -> str:
        """Plugin version."""
        ...
    
    def initialize(self, config: dict[str, Any]) -> None:
        """Initialize plugin with configuration."""
        ...
    
    def cleanup(self) -> None:
        """Cleanup plugin resources."""
        ...


class MemoryStorageProtocol(PluginProtocol, Protocol):
    """Protocol for memory storage backends."""
    
    @abstractmethod
    def save_message(
        self,
        conversation_id: str,
        message: dict[str, Any],
    ) -> str:
        """Save message and return ID."""
        ...
    
    @abstractmethod
    def get_messages(
        self,
        conversation_id: str,
        limit: Optional[int] = None,
    ) -> list[dict[str, Any]]:
        """Retrieve messages for conversation."""
        ...
    
    @abstractmethod
    def delete_conversation(self, conversation_id: str) -> None:
        """Delete entire conversation."""
        ...


class CompressionStrategyProtocol(PluginProtocol, Protocol):
    """Protocol for memory compression strategies."""
    
    @abstractmethod
    def compress(
        self,
        messages: list[dict[str, Any]],
        target_tokens: int,
    ) -> list[dict[str, Any]]:
        """Compress messages to fit target token count."""
        ...


class RetrievalStrategyProtocol(PluginProtocol, Protocol):
    """Protocol for memory retrieval strategies."""
    
    @abstractmethod
    def retrieve_relevant(
        self,
        query: str,
        conversation_id: str,
        top_k: int = 5,
    ) -> list[dict[str, Any]]:
        """Retrieve most relevant messages for query."""
        ...
```

### Plugin Registry

```python
class PluginRegistry:
    """Registry for memory management plugins."""
    
    def __init__(self) -> None:
        self._storage_backends: dict[str, type[MemoryStorageProtocol]] = {
            "memory": InMemoryStorage,
            "redis": RedisStorage,
            "postgres": PostgresStorage,
        }
        self._compression_strategies: dict[str, type[CompressionStrategyProtocol]] = {
            "summarize": SummarizationCompression,
            "truncate": TruncationCompression,
            "importance": ImportanceBasedCompression,
        }
        self._retrieval_strategies: dict[str, type[RetrievalStrategyProtocol]] = {
            "semantic": SemanticRetrieval,
            "recency": RecencyRetrieval,
            "hybrid": HybridRetrieval,
        }
    
    def register_storage(
        self,
        name: str,
        storage_class: type[MemoryStorageProtocol],
    ) -> None:
        """Register custom storage backend."""
        self._storage_backends[name] = storage_class
    
    def register_compression(
        self,
        name: str,
        compression_class: type[CompressionStrategyProtocol],
    ) -> None:
        """Register custom compression strategy."""
        self._compression_strategies[name] = compression_class
    
    def register_retrieval(
        self,
        name: str,
        retrieval_class: type[RetrievalStrategyProtocol],
    ) -> None:
        """Register custom retrieval strategy."""
        self._retrieval_strategies[name] = retrieval_class
    
    def get_storage(self, name: str) -> type[MemoryStorageProtocol]:
        """Get storage backend by name."""
        if name not in self._storage_backends:
            raise KeyError(f"Storage backend '{name}' not found")
        return self._storage_backends[name]


# Global registry
_registry = PluginRegistry()


# Example: User registers custom storage
class CustomS3Storage:
    """Custom S3-backed memory storage."""
    
    @property
    def name(self) -> str:
        return "s3"
    
    @property
    def version(self) -> str:
        return "1.0.0"
    
    def __init__(self, bucket: str, prefix: str = "memory/"):
        self.bucket = bucket
        self.prefix = prefix
        self.s3_client = boto3.client('s3')
    
    def save_message(self, conversation_id: str, message: dict) -> str:
        msg_id = str(uuid.uuid4())
        key = f"{self.prefix}{conversation_id}/{msg_id}.json"
        self.s3_client.put_object(
            Bucket=self.bucket,
            Key=key,
            Body=json.dumps(message),
        )
        return msg_id
    
    # ... other methods


# Register and use
_registry.register_storage("s3", CustomS3Storage)

memory = MemoryManager(
    llm_adapter=user_llm,
    embedding_adapter=user_embed,
    vector_store=user_vector,
    storage="s3://my-bucket/memory/",  # Uses registered S3 storage
)
```

---

## API Design Principles

### Sensible Defaults, Explicit Overrides

```python
# ✅ Good: Minimal setup, works immediately
from context_sdk import MemoryManager

memory = MemoryManager(
    llm_adapter=my_llm,           # Only required: user's LLM
    embedding_adapter=my_embed,    # Only required: user's embeddings
    vector_store=my_vectors,       # Only required: user's vector DB
    # Everything else has sensible defaults
)

conversation = memory.create_conversation("user-123")
conversation.add_message("user", "Hello!")

# Advanced: Override defaults when needed
conversation = memory.create_conversation(
    user_id="user-123",
    max_memory_tokens=2000,        # Default: 1000
    compression_strategy="summarize",  # Default: "truncate"
    retrieval_strategy="hybrid",   # Default: "semantic"
)


# ❌ Bad: Too many required parameters
memory = MemoryManager(
    llm_adapter=my_llm,
    embedding_adapter=my_embed,
    vector_store=my_vectors,
    storage_backend="redis",
    storage_host="localhost",
    storage_port=6379,
    storage_db=0,
    storage_password=None,
    max_memory_tokens=1000,
    compression_enabled=True,
    # ... 20 more parameters
)
```

### Builder Pattern for Complex Memory Configuration

```python
# ✅ Good: Declarative, readable
from context_sdk import ConversationBuilder

conversation = (
    ConversationBuilder(memory_manager)
    .for_user("user-123")
    .with_max_memory_tokens(2000)
    .with_compression("summarize")
    .with_retrieval("hybrid")
    .with_importance_scoring(enabled=True)
    .build()
)

# Also support traditional constructor
conversation = Conversation(
    memory_manager=memory,
    user_id="user-123",
    max_memory_tokens=2000,
    compression_strategy="summarize",
)
```

### Context Managers for Resource Management

```python
# ✅ Good: Automatic cleanup of memory resources
with memory.conversation("user-123") as conv:
    conv.add_message("user", "Hello")
    response = conv.generate_reply()
    # Conversation automatically saved and resources cleaned up


# ✅ Good: Async context manager
async with memory.async_conversation("user-123") as conv:
    await conv.add_message("user", "Hello")
    response = await conv.generate_reply()
```

### Consistent Error Handling

```python
from context_sdk.exceptions import (
    ContextSDKError,        # Base exception
    MemoryStorageError,     # Storage issues
    CompressionError,       # Compression failures
    RetrievalError,         # Retrieval problems
    TokenLimitError,        # Context window exceeded
)

# All SDK exceptions inherit from base
try:
    conversation.add_message("user", "x" * 1_000_000)  # Too large
except TokenLimitError as e:
    print(f"Message too large: {e.token_count}/{e.max_tokens}")
except MemoryStorageError as e:
    print(f"Storage failed: {e}")
except ContextSDKError as e:
    print(f"SDK error: {e}")
```

### Focus on Memory Operations Only

```python
# ✅ Good: Clear separation - we do memory, user does LLM
memory = MemoryManager(llm_adapter=my_llm, ...)

# OUR responsibility: Memory management
conversation.add_message("user", "What's my name?")
relevant_context = conversation.get_relevant_context()
messages_with_context = conversation.build_messages(
    include_system_prompt=True,
    max_tokens=4000,
)

# USER'S responsibility: LLM calls
response_text = my_llm.generate_completion(messages_with_context)

# Back to OUR responsibility: Store response
conversation.add_message("assistant", response_text)


# ❌ Bad: Don't try to hide or wrap LLM calls
# This makes it unclear who's responsible for what
response = conversation.generate_reply()  # Who calls the LLM? Who pays?
```
```

---

## Type Safety & Validation

### 1. Comprehensive Type Hints

```python
from typing import (
    Any,
    Callable,
    Generic,
    Literal,
    Optional,
    TypeVar,
    Union,
    overload,
)

# Type variables for generics
T = TypeVar('T')
ResponseT = TypeVar('ResponseT', bound='BaseResponse')

class Resource(Generic[ResponseT]):
    """Generic resource handler."""
    
    def __init__(
        self,
        client: HTTPClientProtocol,
        response_type: type[ResponseT],
    ) -> None:
        self._client = client
        self._response_type = response_type
    
    def create(self, data: dict[str, Any]) -> ResponseT:
        """Create resource and return typed response."""
        response = self._client.post("/resource", json=data)
        return self._response_type.model_validate(response.json())


# Literal types for strict options
ModelType = Literal["gpt-4", "gpt-3.5-turbo", "claude-3"]

def create_completion(
    prompt: str,
    model: ModelType = "gpt-4",
) -> CompletionResponse:
    """Create completion with type-safe model selection."""
    pass


# Overloads for different return types
@overload
def get_completion(
    prompt: str,
    stream: Literal[False] = False,
) -> CompletionResponse: ...

@overload
def get_completion(
    prompt: str,
    stream: Literal[True],
) -> Iterator[CompletionChunk]: ...

def get_completion(
    prompt: str,
    stream: bool = False,
) -> Union[CompletionResponse, Iterator[CompletionChunk]]:
    """Get completion with type-safe streaming option."""
    if stream:
        return _stream_completion(prompt)
    return _create_completion(prompt)
```

### 2. Pydantic Models for Validation

```python
from pydantic import BaseModel, Field, field_validator, model_validator

class MemoryMessage(BaseModel):
    """Message stored in memory."""
    
    role: Literal["user", "assistant", "system"] = Field(
        ...,
        description="Message role",
    )
    
    content: str = Field(
        ...,
        min_length=1,
        max_length=100_000,
        description="Message content",
    )
    
    timestamp: datetime = Field(
        default_factory=datetime.utcnow,
        description="Message timestamp",
    )
    
    importance_score: float = Field(
        default=1.0,
        ge=0.0,
        le=1.0,
        description="Importance score for retrieval",
    )
    
    metadata: dict[str, Any] = Field(
        default_factory=dict,
        description="Additional metadata",
    )
    
    @field_validator("content")
    @classmethod
    def validate_content(cls, v: str) -> str:
        """Validate content is not empty."""
        if not v.strip():
            raise ValueError("Content cannot be empty or whitespace")
        return v.strip()
    
    @field_validator("importance_score")
    @classmethod
    def validate_importance(cls, v: float) -> float:
        """Ensure importance is between 0 and 1."""
        if not 0.0 <= v <= 1.0:
            raise ValueError("Importance must be between 0.0 and 1.0")
        return v


class ConversationConfig(BaseModel):
    """Configuration for conversation memory."""
    
    max_memory_tokens: int = Field(
        default=1000,
        gt=0,
        le=100_000,
        description="Maximum tokens in context window",
    )
    
    compression_strategy: Literal["truncate", "summarize", "importance"] = Field(
        default="truncate",
        description="How to compress when exceeding limit",
    )
    
    retrieval_strategy: Literal["semantic", "recency", "hybrid"] = Field(
        default="semantic",
        description="How to retrieve relevant context",
    )
    
    enable_importance_scoring: bool = Field(
        default=True,
        description="Whether to score message importance",
    )
    
    @model_validator(mode="after")
    def validate_config(self) -> "ConversationConfig":
        """Validate configuration consistency."""
        if self.compression_strategy == "importance":
            if not self.enable_importance_scoring:
                raise ValueError(
                    "Importance compression requires importance_scoring enabled"
                )
        return self
```

### 3. Protocol-Based Adapters (User-Implemented)

```python
# OUR CODE: Define the contract
from typing import Protocol

class LLMAdapterProtocol(Protocol):
    """What we need from an LLM."""
    
    def generate_completion(
        self,
        messages: list[dict[str, str]],
        **kwargs: Any,
    ) -> str:
        """Generate text completion."""
        ...


class EmbeddingAdapterProtocol(Protocol):
    """What we need for embeddings."""
    
    def embed_text(self, text: str) -> list[float]:
        """Get embedding vector."""
        ...


# USER CODE: They implement for their chosen provider
from openai import OpenAI

class MyOpenAIAdapter:
    """User's adapter - THEIR responsibility."""
    
    def __init__(self, api_key: str):
        self.client = OpenAI(api_key=api_key)
    
    def generate_completion(
        self,
        messages: list[dict[str, str]],
        **kwargs: Any,
    ) -> str:
        """They handle OpenAI API calls."""
        response = self.client.chat.completions.create(
            model=kwargs.get("model", "gpt-4"),
            messages=messages,
        )
        return response.choices[0].message.content
    
    def embed_text(self, text: str) -> list[float]:
        """They handle embedding generation."""
        response = self.client.embeddings.create(
            model="text-embedding-3-small",
            input=text,
        )
        return response.data[0].embedding


# Type checking works!
adapter: LLMAdapterProtocol = MyOpenAIAdapter("sk-...")
memory = MemoryManager(llm_adapter=adapter, ...)
```
```

---

## Error Handling

### Exception Hierarchy

```python
class ContextSDKError(Exception):
    """Base exception for all Context SDK errors."""
    
    def __init__(
        self,
        message: str,
        *,
        code: Optional[str] = None,
        details: Optional[dict[str, Any]] = None,
    ) -> None:
        super().__init__(message)
        self.message = message
        self.code = code
        self.details = details or {}


class MemoryStorageError(ContextSDKError):
    """Raised when memory storage operations fail."""
    
    def __init__(
        self,
        message: str,
        *,
        storage_backend: str,
        operation: str,
        **kwargs: Any,
    ) -> None:
        super().__init__(message, **kwargs)
        self.storage_backend = storage_backend
        self.operation = operation


class TokenLimitError(ContextSDKError):
    """Raised when context window token limit is exceeded."""
    
    def __init__(
        self,
        message: str,
        *,
        token_count: int,
        max_tokens: int,
        **kwargs: Any,
    ) -> None:
        super().__init__(message, **kwargs)
        self.token_count = token_count
        self.max_tokens = max_tokens


class CompressionError(ContextSDKError):
    """Raised when memory compression fails."""
    
    def __init__(
        self,
        message: str,
        *,
        strategy: str,
        **kwargs: Any,
    ) -> None:
        super().__init__(message, **kwargs)
        self.strategy = strategy


class RetrievalError(ContextSDKError):
    """Raised when memory retrieval fails."""
    
    def __init__(
        self,
        message: str,
        *,
        query: str,
        retrieval_strategy: str,
        **kwargs: Any,
    ) -> None:
        super().__init__(message, **kwargs)
        self.query = query
        self.retrieval_strategy = retrieval_strategy


class AdapterError(ContextSDKError):
    """Raised when external adapter operations fail.
    
    Note: We catch and wrap adapter errors, but don't try to fix them.
    The user is responsible for their adapters.
    """
    
    def __init__(
        self,
        message: str,
        *,
        adapter_type: str,
        original_error: Exception,
        **kwargs: Any,
    ) -> None:
        super().__init__(message, **kwargs)
        self.adapter_type = adapter_type
        self.original_error = original_error
```

### Error Handling Best Practices

```python
def add_message(
    self,
    role: str,
    content: str,
) -> MemoryMessage:
    """Add message to conversation memory."""
    try:
        # Validate and create message
        message = MemoryMessage(
            role=role,
            content=content,
            timestamp=datetime.utcnow(),
        )
        
        # Calculate tokens (our responsibility)
        token_count = self._count_tokens(content)
        
        # Check if we exceed limit
        if self._total_tokens + token_count > self.config.max_memory_tokens:
            # Try compression (our responsibility)
            try:
                self._compress_memory()
            except Exception as e:
                raise CompressionError(
                    f"Failed to compress memory: {e}",
                    strategy=self.config.compression_strategy,
                    code="COMPRESSION_FAILED",
                ) from e
        
        # Store message (our responsibility, but uses user's storage)
        try:
            message_id = self._storage.save_message(
                conversation_id=self.id,
                message=message.model_dump(),
            )
            message.id = message_id
        except Exception as e:
            # Wrap storage errors clearly
            raise MemoryStorageError(
                f"Failed to save message: {e}",
                storage_backend=self._storage.name,
                operation="save_message",
                code="STORAGE_FAILED",
            ) from e
        
        # Generate embedding (uses user's adapter)
        try:
            embedding = self._embedding_adapter.embed_text(content)
        except Exception as e:
            # Wrap adapter errors, but don't try to fix them
            raise AdapterError(
                f"Embedding generation failed: {e}",
                adapter_type="embedding",
                original_error=e,
                code="ADAPTER_FAILED",
                details={
                    "suggestion": "Check your embedding adapter implementation",
                    "content_length": len(content),
                },
            ) from e
        
        # Store in vector DB (uses user's vector store)
        try:
            self._vector_store.upsert(
                id=message_id,
                vector=embedding,
                metadata={"role": role, "timestamp": message.timestamp.isoformat()},
            )
        except Exception as e:
            # If vector storage fails, we still have the message in storage
            # Log warning but don't fail the operation
            self._logger.warning(
                f"Vector storage failed: {e}",
                extra={"message_id": message_id},
            )
        
        return message
    
    except ValidationError:
        # Pydantic validation errors - re-raise as-is with helpful message
        raise
    
    except ContextSDKError:
        # Our errors - re-raise as-is
        raise
    
    except Exception as e:
        # Unexpected errors - wrap with context
        raise ContextSDKError(
            f"Unexpected error adding message: {e}",
            code="INTERNAL_ERROR",
            details={
                "role": role,
                "content_length": len(content),
            },
        ) from e


# Usage: Clear error messages help users debug
try:
    conversation.add_message("user", "Hello!")
except TokenLimitError as e:
    print(f"Context window full: {e.token_count}/{e.max_tokens} tokens")
    # User knows they need to clear or compress
except AdapterError as e:
    print(f"Your {e.adapter_type} adapter failed: {e.original_error}")
    # User knows THEIR adapter is the problem, not our SDK
except MemoryStorageError as e:
    print(f"Storage {e.operation} failed on {e.storage_backend}: {e}")
    # User knows their storage backend has issues
```
```

---

## Testing Standards

### Test Structure

```python
"""Tests for conversation memory management."""

import pytest
from unittest.mock import Mock, MagicMock
from datetime import datetime

from context_sdk import MemoryManager, Conversation
from context_sdk.types import MemoryMessage, ConversationConfig
from context_sdk.exceptions import (
    TokenLimitError,
    CompressionError,
    AdapterError,
)


class TestConversationMemory:
    """Test suite for conversation memory operations."""
    
    @pytest.fixture
    def mock_llm_adapter(self) -> Mock:
        """Create mock LLM adapter."""
        adapter = Mock()
        adapter.generate_completion.return_value = "Test response"
        return adapter
    
    @pytest.fixture
    def mock_embedding_adapter(self) -> Mock:
        """Create mock embedding adapter."""
        adapter = Mock()
        adapter.embed_text.return_value = [0.1] * 1536
        adapter.embed_batch.return_value = [[0.1] * 1536] * 5
        return adapter
    
    @pytest.fixture
    def mock_vector_store(self) -> Mock:
        """Create mock vector store."""
        store = Mock()
        store.search.return_value = [
            ("msg-1", 0.95, {"role": "user", "content": "test"}),
            ("msg-2", 0.87, {"role": "assistant", "content": "response"}),
        ]
        return store
    
    @pytest.fixture
    def memory_manager(
        self,
        mock_llm_adapter: Mock,
        mock_embedding_adapter: Mock,
        mock_vector_store: Mock,
    ) -> MemoryManager:
        """Create memory manager with mocked dependencies."""
        return MemoryManager(
            llm_adapter=mock_llm_adapter,
            embedding_adapter=mock_embedding_adapter,
            vector_store=mock_vector_store,
            storage="memory://",  # In-memory for tests
        )
    
    def test_add_message_success(
        self,
        memory_manager: MemoryManager,
        mock_embedding_adapter: Mock,
    ) -> None:
        """Test successfully adding a message."""
        # Arrange
        conversation = memory_manager.create_conversation("user-123")
        
        # Act
        message = conversation.add_message("user", "Hello, world!")
        
        # Assert
        assert message.role == "user"
        assert message.content == "Hello, world!"
        assert isinstance(message.timestamp, datetime)
        mock_embedding_adapter.embed_text.assert_called_once_with("Hello, world!")
    
    def test_add_message_token_limit(
        self,
        memory_manager: MemoryManager,
    ) -> None:
        """Test token limit enforcement."""
        # Arrange
        conversation = memory_manager.create_conversation(
            "user-123",
            config=ConversationConfig(max_memory_tokens=100),
        )
        
        # Add messages until we exceed limit
        for i in range(10):
            conversation.add_message("user", f"Message {i}" * 20)
        
        # Act & Assert
        with pytest.raises(TokenLimitError) as exc_info:
            # This should trigger compression or fail
            conversation.add_message("user", "This exceeds limit" * 100)
        
        assert exc_info.value.token_count > exc_info.value.max_tokens
    
    def test_retrieve_relevant_context(
        self,
        memory_manager: MemoryManager,
        mock_vector_store: Mock,
    ) -> None:
        """Test semantic retrieval of relevant context."""
        # Arrange
        conversation = memory_manager.create_conversation("user-123")
        conversation.add_message("user", "What's my favorite color?")
        
        # Act
        context = conversation.get_relevant_context(
            query="favorite color",
            top_k=5,
        )
        
        # Assert
        assert len(context) > 0
        mock_vector_store.search.assert_called_once()
    
    def test_adapter_failure_handling(
        self,
        memory_manager: MemoryManager,
        mock_embedding_adapter: Mock,
    ) -> None:
        """Test handling of adapter failures."""
        # Arrange
        conversation = memory_manager.create_conversation("user-123")
        mock_embedding_adapter.embed_text.side_effect = Exception("API timeout")
        
        # Act & Assert
        with pytest.raises(AdapterError) as exc_info:
            conversation.add_message("user", "Test message")
        
        assert exc_info.value.adapter_type == "embedding"
        assert "API timeout" in str(exc_info.value.original_error)
    
    @pytest.mark.parametrize(
        "compression_strategy,expected_count",
        [
            ("truncate", 5),
            ("summarize", 2),
            ("importance", 3),
        ],
    )
    def test_compression_strategies(
        self,
        memory_manager: MemoryManager,
        compression_strategy: str,
        expected_count: int,
    ) -> None:
        """Test different compression strategies."""
        # Arrange
        conversation = memory_manager.create_conversation(
            "user-123",
            config=ConversationConfig(
                max_memory_tokens=500,
                compression_strategy=compression_strategy,
            ),
        )
        
        # Add many messages to trigger compression
        for i in range(20):
            conversation.add_message("user", f"Message {i}" * 10)
        
        # Act
        messages = conversation.get_messages()
        
        # Assert - compression should have reduced message count
        assert len(messages) <= expected_count * 2  # Allow some variance


class TestMemoryStorage:
    """Test suite for memory storage backends."""
    
    def test_redis_storage_integration(self) -> None:
        """Integration test with Redis storage."""
        # This would be a real Redis integration test
        memory = MemoryManager(
            llm_adapter=Mock(),
            embedding_adapter=Mock(),
            vector_store=Mock(),
            storage="redis://localhost:6379/0",
        )
        
        conversation = memory.create_conversation("integration-test")
        conversation.add_message("user", "Test message")
        
        # Verify persistence
        retrieved_conv = memory.get_conversation("integration-test")
        assert len(retrieved_conv.get_messages()) == 1
    
    @pytest.mark.benchmark
    def test_retrieval_performance(
        self,
        benchmark,
        memory_manager: MemoryManager,
    ) -> None:
        """Benchmark memory retrieval performance."""
        # Setup: Add 1000 messages
        conversation = memory_manager.create_conversation("perf-test")
        for i in range(1000):
            conversation.add_message("user", f"Message {i}")
        
        # Benchmark retrieval
        result = benchmark(
            conversation.get_relevant_context,
            query="test query",
            top_k=10,
        )
        
        assert len(result) == 10
        # Should complete in < 100ms
        assert benchmark.stats.mean < 0.1
```

### Integration Tests with Real Adapters

```python
"""Integration tests with real external services."""

import pytest
from openai import OpenAI
import pinecone

from context_sdk import MemoryManager


@pytest.mark.integration
@pytest.mark.skipif(
    not os.getenv("OPENAI_API_KEY"),
    reason="Requires OPENAI_API_KEY environment variable"
)
class TestRealIntegration:
    """Integration tests with real external services."""
    
    @pytest.fixture
    def openai_adapter(self):
        """Real OpenAI adapter."""
        from tests.adapters import OpenAIAdapter
        return OpenAIAdapter(api_key=os.getenv("OPENAI_API_KEY"))
    
    @pytest.fixture
    def memory_manager_real(self, openai_adapter):
        """Memory manager with real services."""
        return MemoryManager(
            llm_adapter=openai_adapter,
            embedding_adapter=openai_adapter,
            vector_store=Mock(),  # Still mock vector store for tests
            storage="memory://",
        )
    
    def test_end_to_end_conversation(
        self,
        memory_manager_real: MemoryManager,
    ) -> None:
        """End-to-end test with real LLM."""
        # Create conversation
        conversation = memory_manager_real.create_conversation("e2e-test")
        
        # Add context
        conversation.add_message(
            "user",
            "My name is Alice and I love Python programming"
        )
        conversation.add_message(
            "assistant",
            "Nice to meet you, Alice! Python is a great language."
        )
        
        # Ask question that requires context
        conversation.add_message("user", "What's my name and what do I love?")
        
        # Retrieve context
        context = conversation.get_relevant_context(top_k=2)
        
        # Verify context includes relevant info
        context_text = " ".join([msg["content"] for msg in context])
        assert "Alice" in context_text
        assert "Python" in context_text
```