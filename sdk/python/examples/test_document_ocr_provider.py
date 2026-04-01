from a3s_code import (
    Agent,
    DocumentParserConfig,
    DocumentOcrConfig,
    DocumentOcrProvider,
    DocumentParserRegistry,
    SessionOptions,
)


def ocr_callback(request: dict) -> str | None:
    print("OCR callback path:", request["path"])
    print("OCR callback format:", request["format"])
    print("OCR callback config:", request["config"])

    if request["format"] in {"pdf", "image"}:
        return "Recovered OCR text from Python backend."
    return None


def main() -> None:
    agent = Agent.create("agent.hcl")

    document_parser = DocumentParserConfig()
    ocr = DocumentOcrConfig()
    ocr.enabled = True
    ocr.model = "openai/gpt-4.1-mini"
    ocr.max_images = 2
    ocr.dpi = 144
    document_parser.ocr = ocr

    opts = SessionOptions()
    opts.document_parser_registry = DocumentParserRegistry(document_parser)
    opts.document_ocr_provider = DocumentOcrProvider(
        "python-mock-ocr",
        ocr_callback,
        formats=["pdf", "image"],
        model="openai/gpt-4.1-mini",
    )

    session = agent.session(".", opts)
    tool = session.tool("agentic_parse", {"path": "docs/scanned.pdf"})

    print("tool.output:", tool.output)
    print("tool.metadata:", tool.metadata)
    print("tool.document_runtime:", tool.document_runtime)

    runtime = tool.document_runtime_info
    if runtime and runtime.ocr:
        print("ocr.used:", runtime.ocr.used)
        print("ocr.provider:", runtime.ocr.provider)
        print("ocr.model:", runtime.ocr.model)
        print("ocr.format:", runtime.ocr.format)


if __name__ == "__main__":
    main()
