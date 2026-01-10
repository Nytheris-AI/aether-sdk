from datetime import datetime
from typing import List, Optional, Dict, Any, Literal
from uuid import UUID, uuid4
from enum import Enum
from pydantic import BaseModel, Field

class EntityType(str, Enum):
    PERSON = "Person"
    PROJECT = "Project" 
    GOAL = "Goal"
    DATE = "Date"
    ORGANIZATION = "Organization"
    UNKNOWN = "Unknown"
    INCIDENT = "Incident"
    CODE = "Code"
    RISK = "Risk"

class RelationType(str, Enum):
    WORKS_ON = "WORKS_ON"
    DEADLINE_AT = "DEADLINE_AT"
    PARTNER_WITH = "PARTNER_WITH"
    MENTIONED_IN = "MENTIONED_IN"
    RELATED_TO = "RELATED_TO"
    CALLS = "CALLS"
    CAUSES = "CAUSES"
    MODIFIED = "MODIFIED"
    HAPPENED_AT = "HAPPENED_AT"
    RISK_OF = "RISK_OF"

class BaseSchema(BaseModel):
    class Config:
        from_attributes = True

class IngestionStatus(str, Enum):
    PENDING = "pending"
    PROCESSING = "processing"
    COMPLETED = "completed"
    FAILED = "failed"

class Document(BaseSchema):
    id: str = Field(default_factory=lambda: str(uuid4()))
    project_id: str
    filename: str
    content_type: str
    content: str
    status: IngestionStatus = IngestionStatus.PENDING
    created_at: datetime = Field(default_factory=datetime.utcnow)
    metadata: Dict[str, Any] = {}

class ExtractedEntity(BaseSchema):
    name: str
    type: str 
    description: Optional[str] = None

class ExtractedRelation(BaseSchema):
    source: str
    target: str
    type: str 
    description: Optional[str] = None

class KnowledgeGraph(BaseSchema):
    entities: List[ExtractedEntity]
    relations: List[ExtractedRelation]

class ContextResult(BaseSchema):
    distilled_context: str
    source_references: List[str]
    mode: str = "hybrid" # hybrid, vector, graph
