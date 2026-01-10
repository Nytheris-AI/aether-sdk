from typing import Dict, Any, List, Optional
from aether_sdk.core.protocols import LLMAdapterProtocol

class DistillationResource:
    """
    Handles the synthesis of information (Context Distillation).
    Takes raw retrieved data and generates concise, fact-based answers.
    """
    def __init__(self, llm_adapter: LLMAdapterProtocol):
        self._llm = llm_adapter

    async def distill(self, query: str, context_chunks: List[str], graph_context: List[str] = []) -> str:
        """
        Synthesize an answer from the provided context.
        """
        vector_text = "\n---\n".join(context_chunks)
        graph_text = "\n".join(graph_context) if graph_context else "No graph data."
        
        system_prompt = """
        You are the 'Context Distiller' for an AI Brain.
        Your goal is to synthesize raw information into a high-density, fact-based summary.
        
        Rules:
        1. Answer the user's query using the provided Context.
        2. Combine specific facts from the Graph (names, dates, relations) with semantic nuance from the Vector chunks.
        3. If the context is empty, state that no relevant info was found.
        4. Keep it under 500 words. Format in Markdown.
        """
        
        user_prompt = f"""
        User Query: {query}
        
        [VECTOR CONTEXT]
        {vector_text}
        
        [GRAPH CONTEXT]
        {graph_text}
        
        High-Density Summary:
        """
        
        messages = [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt}
        ]
        
        return self._llm.generate_completion(messages, model="gpt-4o-mini")
