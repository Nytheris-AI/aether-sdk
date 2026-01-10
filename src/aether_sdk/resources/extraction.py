import json
from typing import List, Optional
from aether_sdk.core.protocols import LLMAdapterProtocol
from aether_sdk.types.shared import KnowledgeGraph, ExtractedEntity, ExtractedRelation

class ExtractionResource:
    """
    Handles knowledge extraction (Text -> Graph) using LLM.
    """
    def __init__(self, llm_adapter: LLMAdapterProtocol):
        self._llm = llm_adapter

    async def extract(self, text: str) -> KnowledgeGraph:
        """
        Extracts entities and relations from text.
        """
        system_prompt = """
        You are an expert Data Engineer specializing in Knowledge Graphs.
        Extract structured entities and relations from the provided text.
        
        Entities: 'Person', 'Project', 'Goal', 'Date', 'Organization', 'Event', 'Incident', 'Code', 'Risk'.
        Relations: 'WORKS_ON', 'DEADLINE_AT', 'PARTNER_WITH', 'MENTIONED_IN', 'RELATED_TO', 'CALLS', 'CAUSES', 'MODIFIED', 'RISK_OF'.
        
        Output valid JSON only:
        {
            "entities": [{"name": "Name", "type": "Type", "description": "Desc"}],
            "relations": [{"source": "EntityName", "target": "EntityName", "type": "REL_TYPE", "description": "Desc"}]
        }
        """
        
        # Limit text to avoid context window overflow (simple truncated approach for now)
        truncated_text = text[:8000]
        
        messages = [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": f"Text to analyze:\n{truncated_text}"}
        ]
        
        try:
            response = self._llm.generate_completion(messages, temperature=0.0, response_format={"type": "json_object"})
            
            # Clean up potential markdown marks if adapter doesn't handle it
            clean_response = response.replace("```json", "").replace("```", "").strip()
            
            data = json.loads(clean_response)
            
            entities = [ExtractedEntity(**e) for e in data.get("entities", [])]
            relations = [ExtractedRelation(**r) for r in data.get("relations", [])]
            
            return KnowledgeGraph(entities=entities, relations=relations)
            
        except json.JSONDecodeError:
            # Return empty graph on parse error
            return KnowledgeGraph(entities=[], relations=[])
        except Exception as e:
            # Log or re-raise depending on policy. For SDK library, might be better to log and return empty, or raise specific error.
            # Here we return empty to not break the pipeline.
            print(f"Extraction failed: {e}")
            return KnowledgeGraph(entities=[], relations=[])
