from typing import List, Dict, Any, Optional
import os
import warnings
try:
    import litellm
    from litellm import completion
    # Suppress Pydantic serialization warnings which are common in LiteLLM + Pydantic v2
    warnings.filterwarnings("ignore", message="Pydantic serializer warnings")
except ImportError:
    litellm = None
    completion = None

from aether_sdk.core.protocols import LLMAdapterProtocol

class LiteLLMAdapter(LLMAdapterProtocol):
    """
    Adapter using the `litellm` library to support 100+ LLM providers.
    """
    
    def __init__(
        self, 
        model: str = "gpt-3.5-turbo", 
        api_key: Optional[str] = None, 
        base_url: Optional[str] = None,
        **completion_kwargs: Any
    ):
        """
        Initialize LiteLLM Adapter.
        
        Args:
            model: The model name to use (e.g., 'gpt-4', 'claude-3-opus', 'ollama/llama3', 'openrouter/anthropic/claude-3-opus').
            api_key: Optional API key. If not provided, litellm looks for env vars.
            base_url: Optional base URL for the provider (e.g., for local Ollama or custom endpoints).
            **completion_kwargs: Additional keyword arguments to pass to `litellm.completion` 
                                 (e.g., temperature, max_tokens, api_version, site_url, app_name for OpenRouter).
        """
        if completion is None:
            raise ImportError("litellm is not installed. Please install it with `pip install litellm`.")
            
        self.model = model
        self.api_key = api_key
        self.base_url = base_url
        self.completion_kwargs = completion_kwargs
        
    def generate_completion(
        self,
        messages: List[Dict[str, str]],
        **kwargs: Any,
    ) -> str:
        """
        Generate completion using litellm.
        """
        # Merge init kwargs with call-time kwargs. Call-time kwargs take precedence.
        merged_kwargs = {**self.completion_kwargs, **kwargs}
        
        # Ensure incompatible params are dropped to avoid provider errors
        if "drop_params" not in merged_kwargs:
            merged_kwargs["drop_params"] = True
            
        response = completion(
            model=self.model,
            messages=messages,
            api_key=self.api_key,
            base_url=self.base_url,
            **merged_kwargs
        )
        # Handle response format (OpenAI-like)
        return response.choices[0].message.content

    def generate_structured(
        self,
        messages: List[Dict[str, str]],
        response_schema: Dict[str, Any],
        **kwargs: Any,
    ) -> Dict[str, Any]:
        """
        Generate structured output. 
        Note: LiteLLM supports 'response_format={"type": "json_object"}' for some models.
        For strict schema validation, we might need instructor or similar, but for now 
        we'll pass the schema in system prompt or use JSON mode if model supports it.
        """
        # Merge init kwargs with call-time kwargs.
        merged_kwargs = {**self.completion_kwargs, **kwargs}

        # Simple JSON mode enforcement
        merged_kwargs["response_format"] = {"type": "json_object"}
        
        # Ensure incompatible params are dropped
        if "drop_params" not in merged_kwargs:
            merged_kwargs["drop_params"] = True
            
        # In a real implementation, we might append the schema to the system prompt
        # if the model doesn't support native structured output.
        
        response = completion(
            model=self.model,
            messages=messages,
            api_key=self.api_key,
            base_url=self.base_url,
            **merged_kwargs
        )
        import json
        return json.loads(response.choices[0].message.content)
