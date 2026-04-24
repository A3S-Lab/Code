default_model = "openai/MiniMax-M2.7-highspeed"

providers "openai" {
    apiKey = "sk-MCKpw4RjJTyf7ecYGf01GlgoTaF3iBeuZzdDyyw4kobB5vaj"
    baseUrl = "http://35.220.164.252:3888/v1/"

    models "MiniMax-M2.7-highspeed" {
        name = "MiniMax-M2.7-highspeed"
        reasoning = false
        toolCall = true
    }
}