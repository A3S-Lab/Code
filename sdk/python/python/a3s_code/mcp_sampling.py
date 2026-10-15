"""Internal asyncio dispatch for explicitly host-authorized MCP sampling."""

import json


async def _invoke(callback, server_name, params_json):
    value = await callback(server_name, json.loads(params_json))
    if not isinstance(value, dict):
        raise TypeError("sampling result must be an object")
    return json.dumps(value, allow_nan=False)
