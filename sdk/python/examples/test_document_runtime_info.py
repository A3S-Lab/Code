from a3s_code import Agent


def main() -> None:
    agent = Agent.create("agent.hcl")
    session = agent.session(".")

    tool = session.tool("agentic_parse", {"path": "docs/scanned.pdf"})

    print("metadata_json:", tool.metadata_json)
    print("metadata:", tool.metadata)
    print("document_runtime_json:", tool.document_runtime_json)
    print("document_runtime:", tool.document_runtime)

    runtime = tool.document_runtime_info
    if runtime and runtime.ocr:
        print("ocr.used:", runtime.ocr.used)
        print("ocr.provider:", runtime.ocr.provider)
        print("ocr.model:", runtime.ocr.model)
        print("ocr.dpi:", runtime.ocr.dpi)

    query_tool = session.tool(
        "agentic_parse",
        {"path": "docs/scanned.pdf", "query": "overview"},
    )
    print("agentic_parse_llm_blocks:", query_tool.agentic_parse_llm_blocks)
    for block in query_tool.agentic_parse_llm_blocks_info:
        location = block.location.display if block.location else None
        print("llm_block:", block.index, block.kind, block.label, location)


if __name__ == "__main__":
    main()
