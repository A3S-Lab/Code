default_model = "openai/kimi-k2.5"

providers {
    name = "openai"
    api_key = env("A3S_API_KEY")
    base_url = env("A3S_BASE_URL")

    models {
        id = "kimi-k2.5"
        name = "Kimi"
        reasoning = true
    }
}
