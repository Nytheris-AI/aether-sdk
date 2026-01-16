from enum import Enum

class MemoryAction(Enum):
    ADD = "add"
    UPDATE = "update"
    DELETE = "delete"
    IGNORE = "ignore"

from aether_sdk.core.protocols import LLMAdapterProtocol

class PolicyRouter:
    """
    Decides the fate of a perforated/atomized memory unit.
    Routes to Episodic, Semantic, or ignores based on policy.
    """
    
    def __init__(self, llm_client: LLMAdapterProtocol = None):
        self.llm_client = llm_client

    async def decide_action(self, content: str) -> MemoryAction:
        """
        Evaluate if the content is worth storing.
        """
        # If no LLM, fallback to simple heuristic (always add if not empty)
        if not self.llm_client:
            return MemoryAction.ADD if content.strip() else MemoryAction.IGNORE
            
        # LLM-based Policy Check (Simplified Reflection)
        # "Is this information useful, novel, and not essentially noise?"
        prompt = f"""
        Analyze the following text and decide if it should be stored in long-term memory.
        
        Text: "{content}"
        
        Criteria:
        - IGNORE if it is phatic (e.g., "Hello", "Thanks"), vague, or useless.
        - ADD if it contains a fact, preference, event, or useful context.
        
        Return ONLY one word: ADD or IGNORE.
        """
        
        messages = [{"role": "user", "content": prompt}]
        try:
            response = await self.llm_client.chat_completion(messages, temperature=0.0)
            decision = response.strip().upper()
            if "IGNORE" in decision:
                return MemoryAction.IGNORE
            return MemoryAction.ADD
        except Exception:
            return MemoryAction.ADD

