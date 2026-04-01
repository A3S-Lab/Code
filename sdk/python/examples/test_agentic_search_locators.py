"""Inspect typed agentic_search match locators from the Python SDK."""

from a3s_code import Agent


def main() -> None:
    agent = Agent.create("agent.hcl")
    session = agent.session(".")

    tool = session.tool("agentic_search", {"query": "overview", "mode": "fast"})

    for result in tool.agentic_search_results_info:
        print("result:", result.path, result.file_type, result.relevance)
        for match in result.matches:
            print("  match:", match.line_number, match.locator, match.content)
            if match.context_before:
                print("    before:", match.context_before[-1])
            if match.context_after:
                print("    after:", match.context_after[0])

    deep = session.tool("agentic_search", {"query": "overview", "mode": "deep"})
    for result in deep.agentic_search_results_info:
        print("deep:", result.path, result.file_type, result.relevance)
        for sampled in result.sampled_lines:
            print(
                "  sampled:",
                sampled.line_number,
                sampled.locator,
                sampled.distance,
                sampled.weight,
            )


if __name__ == "__main__":
    main()
