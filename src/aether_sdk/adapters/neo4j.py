from typing import List, Dict, Any, Optional
import os
from neo4j import GraphDatabase
from aether_sdk.core.protocols import GraphStoreProtocol

class Neo4jAdapter:
    """
    Implementation of GraphStoreProtocol for Neo4j.
    """
    def __init__(self, uri: str = None, username: str = None, password: str = None, database: str = "neo4j"):
        self.uri = uri or os.getenv("NEO4J_URI")
        self.username = username or os.getenv("NEO4J_USERNAME")
        self.password = password or os.getenv("NEO4J_PASSWORD")
        self.database = database or os.getenv("NEO4J_DATABASE", "neo4j")
        
        if self.uri and self.username and self.password:
            self.driver = GraphDatabase.driver(
                self.uri,
                auth=(self.username, self.password)
            )
        else:
            self.driver = None
            
    def close(self):
        if self.driver:
            self.driver.close()

    def execute_query(self, query: str, parameters: Optional[Dict[str, Any]] = None) -> List[Dict[str, Any]]:
        """Executes a Cypher query."""
        if not self.driver:
             # Or raise error
             return []
             
        records, summary, keys = self.driver.execute_query(query, parameters, database_=self.database)
        return [record.data() for record in records]
        
    def upsert_node(
        self,
        project_id: str,
        name: str,
        type: str,
        properties: Optional[Dict[str, Any]] = None
    ) -> None:
        """Upsert a node (Entity) into Neo4j."""
        properties = properties or {}
        # Construct Cypher to set properties dynamic
        # Note: We hardcode 'description' if present in properties or root for back-compat
        
        desc = properties.get("description", "")
        
        query = """
        MERGE (e:Entity {name: $name, project_id: $project_id})
        ON CREATE SET e.type = $type, e.description = $description, e.created_at = datetime()
        ON MATCH SET e.last_seen = datetime(), e.description = $description
        RETURN e
        """
        # Note: A real implementation would iterate properties and SET them.
        # But Cypher injections are tricky.
        # Minimal viable implementation for now:
        params = {
            "name": name, 
            "project_id": project_id, 
            "type": type, 
            "description": desc
        }
        self.execute_query(query, params)

    def upsert_relation(
        self,
        project_id: str,
        source: str,
        target: str,
        type: str,
        properties: Optional[Dict[str, Any]] = None
    ) -> None:
        """Upsert a relation (Edge) into Neo4j."""
        properties = properties or {}
        desc = properties.get("description", "")
        
        # We need to sanitize 'type' as it cannot be parameterized in Cypher relationships usually (unless using APOC)
        # Assuming type is safe Enum or validated string.
        # TODO: Add validation.
        
        query = f"""
        MERGE (s:Entity {{name: $source_name, project_id: $project_id}})
        MERGE (t:Entity {{name: $target_name, project_id: $project_id}})
        MERGE (s)-[r:{type}]->(t)
        ON CREATE SET r.description = $description, r.created_at = datetime()
        RETURN type(r)
        """
        params = {
            "project_id": project_id,
            "source_name": source,
            "target_name": target, 
            "description": desc
        }
        self.execute_query(query, params)

    def create_entity_node(self, project_id: str, name: str, type: str, description: str = None) -> None:
        """Deprecated helper: Delegates to upsert_node."""
        self.upsert_node(project_id, name, type, {"description": description})

    def create_relation(self, project_id: str, source: str, target: str, type: str, description: str = None) -> None:
        """Deprecated helper: Delegates to upsert_relation."""
        self.upsert_relation(project_id, source, target, type, {"description": description})
