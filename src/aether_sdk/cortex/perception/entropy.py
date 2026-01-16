import re
from typing import List

class SimpleEntropyFilter:
    """
    Lightweight Rule-Based Filter.
    Rule: KEEP if (Has Named Entity) AND (Cosine Sim < 0.8).
    """
    
    def __init__(self, embeddings_client):
        self.embeddings = embeddings_client
        self.history_vectors = []
        
    async def check(self, content: str) -> bool:
        """
        Determines if content should be passed to the memory system.
        """
        # 1. Entity Check (Heuristic: Capitalized words in middle of sentence)
        # Matches words starting with UpperCase that are not at start of sentence (rough approx) or just any UpperCase > 1 char
        # excluding common start-of-sentence stopwords is hard without NLP lib.
        # Let's settle for: Any Capitalized Word that is likely a Proper Noun.
        has_entity = bool(re.search(r'[A-Z][a-z]+', content))
        
        # 2. Similarity Check (Redundancy)
        is_redundant = False
        if self.history_vectors:
             # Calculate embedding
             try:
                 vecs = await self.embeddings.embed_batch([content])
                 if vecs and len(vecs) > 0:
                     current_vec = vecs[0]
                     
                     # Check max sim against last 3
                     for past_vec in self.history_vectors[-3:]:
                         sim = self._cosine_similarity(current_vec, past_vec)
                         if sim > 0.8:
                             is_redundant = True
                             break
                     
                     # Update history
                     self.history_vectors.append(current_vec)
                     if len(self.history_vectors) > 10:
                         self.history_vectors.pop(0)
             except Exception:
                 # If embedding fails, default to allow if entity present
                 pass
        else:
             # First message, just embed and store
             try:
                 vecs = await self.embeddings.embed_batch([content])
                 if vecs:
                    self.history_vectors.append(vecs[0])
             except Exception:
                 pass
             
        return has_entity and not is_redundant

    def _cosine_similarity(self, v1, v2):
        if not v1 or not v2: return 0.0
        return sum(a*b for a,b in zip(v1,v2))
