import tempfile
import os
from io import BytesIO
from typing import Optional, Union
# from docling.document_converter import DocumentConverter # Lazy load this!

class ParsingResource:
    """
    Handles document parsing (File/Bytes/URL -> Markdown).
    Uses 'docling' under the hood.
    """
    def __init__(self):
        # Lazy load converter? Or eager?
        # Docling can be heavy, but for SDK usage eager is usually fine or lazily on first use.
        self._converter = None

    @property
    def converter(self):
        if self._converter is None:
            # Lazy Import for performance
            from docling.document_converter import DocumentConverter
            self._converter = DocumentConverter()
        return self._converter

    def parse_bytes(self, content: bytes, mime_type: str) -> str:
        """
        Parses binary content into Markdown.
        """
        # Map MIME types to extensions for Docling's format detection
        suffix_map = {
            "application/pdf": ".pdf",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document": ".docx",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation": ".pptx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet": ".xlsx",
            "image/png": ".png",
            "image/jpeg": ".jpg",
            "image/tiff": ".tiff",
            "image/bmp": ".bmp",
            "image/webp": ".webp",
            "text/html": ".html",
            "application/xhtml+xml": ".xhtml",
            "text/markdown": ".md",
            "text/csv": ".csv",
            "text/plain": ".txt",
        }
        suffix = suffix_map.get(mime_type, ".txt")
        
        with tempfile.NamedTemporaryFile(delete=False, suffix=suffix) as tmp:
            tmp.write(content)
            tmp_path = tmp.name
            
        try:
            res = self.converter.convert(tmp_path)
            return res.document.export_to_markdown()
        finally:
            if os.path.exists(tmp_path):
                os.remove(tmp_path)
                
    def parse_file(self, file_path: str) -> str:
        """Parses a local file."""
        res = self.converter.convert(file_path)
        return res.document.export_to_markdown()
