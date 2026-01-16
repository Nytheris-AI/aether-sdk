from typing import Optional, Any

from aether_sdk.core.protocols import LLMAdapterProtocol, EmbeddingAdapterProtocol


from aether_sdk.cortex.memory.orchestrator import AgentMemory

import json
import re
from typing import Optional, Any, List, Dict

from aether_sdk.core.protocols import LLMAdapterProtocol, EmbeddingAdapterProtocol
from aether_sdk.cortex.prompts import SYSTEM_PROMPT, TOOL_SCHEMAS
from aether_sdk.cortex.perception.entropy import SimpleEntropyFilter
from aether_sdk.cortex.monitor import BrainEvent, BrainMonitor

class AetherBrain:
    """
    Zero-Shot LightAgent Brain.
    Uses a System Prompt to enforce a Recall -> Reason -> Act loop.
    """
    
    def __init__(
        self,
        llm: LLMAdapterProtocol,
        embeddings: EmbeddingAdapterProtocol,
        memory_store: Any = None,
        monitors: List[BrainMonitor] = None
    ):
        self.llm = llm
        self.embeddings = embeddings
        
        # Initialize Memory Store if not provided
        if memory_store is None:
             pass
        self.memory_store = memory_store
        
        self.monitors = monitors or []
        
        self.entropy_filter = SimpleEntropyFilter(embeddings)

        
        # Conversation Context
        self.history: List[Dict[str, str]] = [
            {"role": "system", "content": SYSTEM_PROMPT}
        ]

    def _emit_event(self, type: str, content: str, metadata: Dict = None):
        """Helper to notify all monitors."""
        if not self.monitors: return
        event = BrainEvent(type=type, content=content, metadata=metadata)
        for monitor in self.monitors:
            monitor.log_event(event)

    async def run(self, user_input: str) -> str:
        """
        Main Agent Loop:
        1. Append User Input.
        2. LLM Inference (Reasoning).
        3. Parsed <tool_call>? Execute & Recurse.
        4. No tool? Return Answer.
        """
        self.history.append({"role": "user", "content": user_input})
        
        # Max turns to prevent infinite loops
        for _ in range(5):
            response = await self.llm.chat_completion(self.history, temperature=0.0)
            
            # Helper to extract content if response is complex object, 
            # assuming adapter returns str for now as per previous code
            response_text = str(response) 
            
            # 1. Check for Tool Calls
            tool_call_match = re.search(r"<tool_call>(.*?)</tool_call>", response_text, re.DOTALL)
            
            # Emit Thought Event (Everything before/around tool call is reasoning)
            thought_text = re.search(r"<think>(.*?)</think>", response_text, re.DOTALL)
            if thought_text:
                self._emit_event("THOUGHT", thought_text.group(1).strip())
            
            if tool_call_match:
                # We have a tool call
                # Append the "Thought" + "Tool Call" to history so model sees its own reasoning
                self.history.append({"role": "assistant", "content": response_text})
                
                tool_json_str = tool_call_match.group(1).strip()
                # Clean up markdown code blocks if present (common model artifact)
                tool_json_str = re.sub(r"^```json\s*", "", tool_json_str)
                tool_json_str = re.sub(r"^```\s*", "", tool_json_str)
                tool_json_str = re.sub(r"\s*```$", "", tool_json_str)
                
                try:
                    tool_calls = json.loads(tool_json_str)
                    if isinstance(tool_calls, list):
                        for tool in tool_calls:
                            self._emit_event("TOOL_CALL", "Calling Tool", {"tool_name": tool['name'], "tool_args": tool['arguments']})
                            tool_result = await self._execute_tool(tool)
                            self._emit_event("TOOL_RESULT", str(tool_result), {"tool_name": tool['name']})
                            
                            self.history.append({
                                "role": "user",  # Treating Observation as User message for simplicity or system
                                "content": f"Observation for {tool['name']}: {tool_result}"
                            })
                    else:
                        # Handle single object case if model messes up array format
                         tool = tool_calls
                         self._emit_event("TOOL_CALL", "Calling Tool", {"tool_name": tool['name'], "tool_args": tool['arguments']})
                         tool_result = await self._execute_tool(tool)
                         self._emit_event("TOOL_RESULT", str(tool_result), {"tool_name": tool['name']})
                         
                         self.history.append({
                            "role": "user", 
                            "content": f"Observation: {tool_result}"
                        })
                        
                except Exception as e:
                     error_msg = f"System Error: Invalid JSON in tool call. {e}"
                     self._emit_event("ERROR", error_msg)
                     self.history.append({
                        "role": "user",
                        "content": error_msg
                    })
                
                # Loop continues to get the next step/final answer
                continue
            
            else:
                # No tool call -> Final Answer
                self.history.append({"role": "assistant", "content": response_text})
                
                # Cleanup output for user (remove think tags)
                final_answer = re.sub(r"<think>.*?</think>", "", response_text, flags=re.DOTALL).strip()
                self._emit_event("ANSWER", final_answer)
                return final_answer
                
        return "Error: Maximum agent turns exceeded."

    async def _execute_tool(self, tool_call: Dict[str, Any]) -> str:
        """
        Dispatch tool calls to implementation.
        """
        name = tool_call.get("name")
        args = tool_call.get("arguments", {})
        
        if name == "retrieve_memory":
            return await self._retrieve_memory(args.get("query"))
        elif name == "add_memory":
            return await self._add_memory(args.get("content"), args.get("entities"))
        elif name == "update_memory":
            return await self._update_memory(args.get("memory_id"), args.get("new_content"))
        elif name == "delete_memory":
            return await self._delete_memory(args.get("memory_id"))
        else:
            return f"Error: Unknown tool '{name}'"

    # --- Tool Implementations (Placeholders for next phase) ---
    
    async def _retrieve_memory(self, query: str) -> str:
        # High-Scale: Use Unified AgentMemory
        if not self.memory_store:
            return "Memory store not initialized."
            
        # Retrieval now uses the unified orchestrator (Hybrid Vector + Graph)
        memories = await self.memory_store.retrieve(query)
        
        if not memories:
            return "No relevant memories found."
            
        return json.dumps([{"id": m.id, "content": m.content, "type": m.memory_type.value} for m in memories], default=str)
        
    async def _add_memory(self, content: str, entities: List[str] = None) -> str:
        # Phase 7: Entropy Filter Rule
        if self.entropy_filter:
            should_keep = await self.entropy_filter.check(content)
            if not should_keep:
                return "Memory ignored: Low entropy (No named entities or redundant)."
                
        # High-Scale: Use Unified AgentMemory (Buffered Async)
        if self.memory_store:
            # We pass entities as metadata if needed
            metadata = {"entities": entities} if entities else {}
            ack_id = await self.memory_store.add_memory(content, metadata)
            return f"Memory ingestion started. ACK ID: {ack_id}"
        return "Memory store not available."
        
    async def _update_memory(self, memory_id: str, new_content: str) -> str:
        # Placeholder for MVP
        return "Memory updated (Simulated)."
        
    async def _delete_memory(self, memory_id: str) -> str:
        # Placeholder for MVP
        return "Memory deleted (Simulated)."

