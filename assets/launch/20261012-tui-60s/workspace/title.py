def title(value: str) -> str:
    trimmed = value.strip()
    if not trimmed:
        return "Untitled"
    return trimmed
