---
name: http-tool-example
description: Example of HTTP API-based tools
version: 1.0.0
tools:
  - name: weather
    description: Get weather information for a city using OpenWeatherMap API
    backend:
      type: http
      url: https://api.openweathermap.org/data/2.5/weather
      method: GET
      headers:
        Accept: application/json
      body_template: |
        {
          "q": "${city}",
          "appid": "${api_key}",
          "units": "metric"
        }
      timeout_ms: 10000
    parameters:
      type: object
      properties:
        city:
          type: string
          description: City name (e.g., "London", "Tokyo")
        api_key:
          type: string
          description: OpenWeatherMap API key
      required:
        - city
        - api_key

  - name: translate
    description: Translate text using a translation API
    backend:
      type: http
      url: https://api.example.com/translate
      method: POST
      headers:
        Content-Type: application/json
        Authorization: Bearer ${api_token}
      body_template: |
        {
          "text": "${text}",
          "source_lang": "${source_lang}",
          "target_lang": "${target_lang}"
        }
      timeout_ms: 15000
    parameters:
      type: object
      properties:
        text:
          type: string
          description: Text to translate
        source_lang:
          type: string
          description: Source language code (e.g., "en", "zh")
        target_lang:
          type: string
          description: Target language code (e.g., "en", "zh")
        api_token:
          type: string
          description: API authentication token
      required:
        - text
        - source_lang
        - target_lang
        - api_token
---

# HTTP Tool Examples

HTTP tools make API calls to external services.

## Features

- **RESTful APIs**: Support GET, POST, PUT, DELETE, etc.
- **Custom headers**: Set authentication, content-type, etc.
- **Body templating**: Use `${arg_name}` in JSON body templates
- **Timeout control**: Configure request timeout in milliseconds

## Usage

HTTP tools automatically make API calls when invoked. The response is returned as tool output.

## Security Note

API keys and tokens should be passed as parameters, not hardcoded in the skill definition.
