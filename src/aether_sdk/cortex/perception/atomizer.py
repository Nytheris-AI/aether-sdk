from typing import Any, Dict, List, Optional
import json
from aether_sdk.core.protocols import LLMAdapterProtocol

class AtomizerProtocol:
    """Protocol for breaking down text into atomic facts."""
    
    async def atomize(self, text: str) -> List[Dict[str, Any]]:
        """Convert text into atomic facts."""
        ...

class SemanticAtomizer(AtomizerProtocol):
    """
    De-linearizes text into atomic units using an LLM.
    Adapted from SimpleMem's MemoryBuilder.
    """
    
    def __init__(self, llm_client: LLMAdapterProtocol):
        self.llm_client = llm_client

    async def atomize(self, text: str, context: str = "") -> List[Dict[str, Any]]:
        """
        Extracts atomic entries from text using LLM.
        """
        dict_schema = {
            "type": "json_object"
        }
        
        prompt = self._build_prompt(text, context)
        
        messages = [
            {
                "role": "system",
                "content": "You are a professional information extraction assistant. Extract structured, unambiguous information. Output valid JSON."
            },
            {
                "role": "user",
                "content": prompt
            }
        ]
        
        response = await self.llm_client.chat_completion(
            messages=messages,
            response_format=dict_schema,
            temperature=0.1
        )
        
        return self._parse_response(response)

    def _build_prompt(self, text: str, context: str) -> str:
        return f"""
Your task is to extract atomic memory entries from the following text.

[Context]
{context}

[Text to Process]
{text}

[Requirements]
1. **Atomic**: Each entry must be self-contained.
2. **No Pronouns**: Replace he/she/it/they with specific names.
3. **Absolute Time**: Convert "tomorrow" to absolute dates if possible.
4. **Lossless**: Capture all names, locations, and key details.

[Output Format]
Return a JSON array of objects:
```json
[
  {{
    "content": "Complete unambiguous sentence.",
    "keywords": ["tag1", "tag2"],
    "entities": ["Alice", "Bob", "Project X"],
    "type": "fact" | "event" | "observation"
  }}
]
```
"""

    def _parse_response(self, response: str) -> List[Dict[str, Any]]:
        try:
            # Basic cleanup if the LLM wraps code in ```json ... ```
            clean_res = response.strip()
            if clean_res.startswith("```json"):
                clean_res = clean_res[7:]
            if clean_res.endswith("```"):
                clean_res = clean_res[:-3]
            
            data = json.loads(clean_res)
            
            if isinstance(data, list):
                return data
            elif isinstance(data, dict) and "entries" in data:
                 return data["entries"]
            else:
                # Fallback or error
                return []
        except Exception:
            # In production, log error
            return []

