import json
import time
from typing import Dict, Any, Optional
from abc import ABC, abstractmethod
from dataclasses import dataclass, asdict

# Optional Rich Import
try:
    from rich.console import Console
    from rich.panel import Panel
    from rich.text import Text
    from rich.theme import Theme
    HAS_RICH = True
except ImportError:
    HAS_RICH = False

@dataclass
class BrainEvent:
    type: str  # THOUGHT, TOOL_CALL, TOOL_RESULT, ANSWER, ERROR
    content: str
    metadata: Optional[Dict[str, Any]] = None
    timestamp: float = 0.0
    
    def __post_init__(self):
        if self.timestamp == 0.0:
            self.timestamp = time.time()

class BrainMonitor(ABC):
    """
    Observer protocol for AetherBrain.
    """
    @abstractmethod
    def log_event(self, event: BrainEvent):
        pass

class JSONLMonitor(BrainMonitor):
    """
    Logs events to a JSONL file.
    """
    def __init__(self, filepath: str = "brain_logs.jsonl"):
        self.filepath = filepath
    
    def log_event(self, event: BrainEvent):
        with open(self.filepath, "a", encoding="utf-8") as f:
            f.write(json.dumps(asdict(event)) + "\n")

class RichConsoleMonitor(BrainMonitor):
    """
    Visualizes events using Rich.
    """
    def __init__(self):
        if not HAS_RICH:
            print("Warning: 'rich' library not found. Console monitoring disabled.")
            return
            
        custom_theme = Theme({
            "thought": "dim cyan",
            "tool": "bold yellow",
            "result": "green",
            "answer": "bold white",
            "error": "bold red"
        })
        self.console = Console(theme=custom_theme)
        
    def log_event(self, event: BrainEvent):
        if not HAS_RICH: return
        
        if event.type == "THOUGHT":
            # Strip tags for display if present, or just show raw
            self.console.print(f"[thought]Running Thought Process...[/thought]")
            self.console.print(Panel(event.content, title="Thinking", border_style="cyan", padding=(0,1)))
            
        elif event.type == "TOOL_CALL":
            self.console.print(f"[tool]▶ Tool Call: {event.metadata.get('tool_name', 'unknown')}[/tool]")
            self.console.print(f"  Args: {event.metadata.get('tool_args')}")
            
        elif event.type == "TOOL_RESULT":
            self.console.print(f"[result]◀ Result ({event.metadata.get('tool_name')}):[/result] {event.content[:100]}...")
            
        elif event.type == "ANSWER":
            self.console.print(Panel(event.content, title="Agent Answer", border_style="green"))
            
        elif event.type == "ERROR":
            self.console.print(f"[error]! Error:[/error] {event.content}")
