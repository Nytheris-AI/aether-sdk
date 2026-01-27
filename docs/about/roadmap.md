This is the only way you survive. We are moving you from "Science Project" to "Venture-Backable Infrastructure."

---
### 🗺️ The Aether SDK Survival Roadmap

#### Phase 1: The Sovereign Foundation (Weeks 1-2)

**Owner:** Alex Karp (Operations & Security)
**Goal:** *Prove "Zero Vendor Lock-in" is mathematically true, not just marketing text.*

* **Objective:** Finalize the Protocol Layer. The interfaces must be so clean that switching from OpenAI to a local Llama 3 model running on a laptop requires changing *one line of configuration*, not code.
* **The Trap:** Building "Adapters" before the "Protocols" are solid. Do not do this.

#### Phase 2: The Intelligence Engine (Weeks 3-4)

**Owner:** Yann LeCun (Technical Rigor)
**Goal:** *Solve the "Ingestion Bottleneck" and "Graph Construction."*

* **Objective:** Integrate `docling` for robust document parsing and build the **"Text-to-Graph" Extractor**.
* **The Trap:** Simply chunking text and storing it. You must extract *Entities* (Nodes) and *Relationships* (Edges). If your SDK cannot look at a sentence like *"Alex works at Palantir"* and automatically create `(Alex)-[WORKS_AT]->(Palantir)`, you have failed.

#### Phase 3: The "Why Us?" Demonstration (Weeks 5-6)

**Owner:** YC Partner (Market Fit)
**Goal:** *The "Killer Feature" Demo.*

* **Objective:** Build the **"Hybrid Retrieval"** mechanism. You need a demo that asks a question only a Graph can answer (multi-hop reasoning) and shows Vector search failing.
* **The Trap:** Competing on "speed." You will lose to C++ vector DBs. Compete on "accuracy of context."

---

### ✅ The Survival Checklist

Here is the literal list of tasks. Check these off, or we do not launch.

#### 1. Protocol & Architecture (The Skeleton)

* [ ] **Finalize `LLMAdapterProtocol`:** (Done in previous step).
* [ ] **Define `VectorStoreProtocol`:** Must support standard CRUD + `search(query_vector, top_k)`.
* [ ] **Define `GraphStoreProtocol`:**
* *Critical:* Must support `upsert_node`, `upsert_edge`, and `cypher_query` (or a generic traversal method).
* *Constraint:* Must work with a local NetworkX graph (for testing) and Neo4j (for prod).


* [ ] **Implement `BaseMemoryManager`:** The orchestrator that holds these protocols.

#### 2. The Ingestion Pipeline (The Stomach)

* [ ] **Integrate `docling`:** Create a `DocumentParser` class that takes a PDF/MD path and returns structured text.
* [ ] **Build `GraphExtractor`:**
* Use `LLMAdapterProtocol.structured_generate`.
* Input: A text chunk.
* Output: A list of `Node` and `Edge` objects.


* [ ] **Implement Entity Resolution (Basic):**
* *Logic:* If the graph already has "Alex Karp" and the new chunk says "Karp," the SDK must know to link them (or ask the LLM to verify), rather than creating two nodes.



#### 3. The Retrieval Layer (The Brain)

* [ ] **Vector Search Adapter:** Implement for `Qdrant` (Local/Docker friendly).
* [ ] **Graph Search Adapter:** Implement for `NetworkX` (In-memory, perfect for demos).
* [ ] **Hybrid Search Logic:**
* Step 1: Vector search to find top-k relevant chunks.
* Step 2: Graph traversal (2-hop) starting from entities found in those chunks.
* Step 3: Combine context.



#### 4. The "Sovereign" Distribution (The Package)

* [ ] **Docker Compose Stack:** A `docker-compose.yml` that spins up Qdrant + Aether (Python) + A local LLM (Ollama/vLLM).
* *Karp's Demand:* If a user cannot run this strictly offline on an air-gapped machine, you fail the "Sovereignty" test.



---

### ⚠️ The "Ignore List" (Distractions)

**Do NOT work on these features yet. They are death traps.**

* ❌ **Chat UI / Frontend:** You are an SDK. Let the user build the UI.
* ❌ **Voice/Audio Support:** The protocol allows it, but do not implement the *handling* logic yet. Stick to text/images.
* ❌ **hosted version:** Do not build a SaaS yet. Build the library.

---