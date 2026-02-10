/**
 * OpenAI-Compatible Types and Conversion Layer
 *
 * This module provides OpenAI-compatible type definitions and conversion
 * functions to allow seamless integration with OpenAI-style APIs.
 *
 * OpenAI Message Format:
 * {
 *   role: "user" | "assistant" | "system" | "tool",
 *   content: string | ContentPart[],
 *   name?: string,
 *   tool_calls?: ToolCall[],
 *   tool_call_id?: string,
 * }
 */

import type {
  Message,
  MessageRole,
  ToolCall,
  GenerateResponse,
  GenerateChunk,
  FinishReason,
  Usage,
} from './client.js';

// ============================================================================
// OpenAI-Compatible Types
// ============================================================================

/**
 * OpenAI-compatible message role
 */
export type OpenAIRole = 'user' | 'assistant' | 'system' | 'tool' | 'function';

/**
 * OpenAI-compatible text content part
 */
export interface OpenAITextContent {
  type: 'text';
  text: string;
}

/**
 * OpenAI-compatible image content part
 */
export interface OpenAIImageContent {
  type: 'image_url';
  image_url: {
    url: string;
    detail?: 'auto' | 'low' | 'high';
  };
}

/**
 * OpenAI-compatible content part (union type)
 */
export type OpenAIContentPart = OpenAITextContent | OpenAIImageContent;

/**
 * OpenAI-compatible tool call
 */
export interface OpenAIToolCall {
  id: string;
  type: 'function';
  function: {
    name: string;
    arguments: string;
  };
}

/**
 * OpenAI-compatible message
 */
export interface OpenAIMessage {
  role: OpenAIRole;
  content: string | OpenAIContentPart[] | null;
  name?: string;
  tool_calls?: OpenAIToolCall[];
  tool_call_id?: string;
}

/**
 * OpenAI-compatible chat completion choice
 */
export interface OpenAIChoice {
  index: number;
  message: OpenAIMessage;
  finish_reason: 'stop' | 'length' | 'tool_calls' | 'content_filter' | null;
}

/**
 * OpenAI-compatible usage statistics
 */
export interface OpenAIUsage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
}

/**
 * OpenAI-compatible chat completion response
 */
export interface OpenAIChatCompletion {
  id: string;
  object: 'chat.completion';
  created: number;
  model: string;
  choices: OpenAIChoice[];
  usage?: OpenAIUsage;
}

/**
 * OpenAI-compatible streaming chunk delta
 */
export interface OpenAIDelta {
  role?: OpenAIRole;
  content?: string | null;
  tool_calls?: Partial<OpenAIToolCall>[];
}

/**
 * OpenAI-compatible streaming choice
 */
export interface OpenAIStreamChoice {
  index: number;
  delta: OpenAIDelta;
  finish_reason: 'stop' | 'length' | 'tool_calls' | 'content_filter' | null;
}

/**
 * OpenAI-compatible streaming chunk
 */
export interface OpenAIChatCompletionChunk {
  id: string;
  object: 'chat.completion.chunk';
  created: number;
  model: string;
  choices: OpenAIStreamChoice[];
}

// ============================================================================
// Conversion Functions: OpenAI -> A3S
// ============================================================================

/**
 * Convert OpenAI role to A3S MessageRole
 * Since proto now uses OpenAI-compatible string roles, this is a simple passthrough
 */
export function openAIRoleToA3S(role: OpenAIRole): MessageRole {
  switch (role) {
    case 'user':
      return 'user';
    case 'assistant':
      return 'assistant';
    case 'system':
      return 'system';
    case 'tool':
    case 'function':
      return 'tool';
    default:
      return 'user';
  }
}

/**
 * Convert OpenAI message to A3S Message
 */
export function openAIMessageToA3S(msg: OpenAIMessage): Message {
  let content: string;

  if (typeof msg.content === 'string') {
    content = msg.content;
  } else if (Array.isArray(msg.content)) {
    // Extract text from content parts
    content = msg.content
      .filter((part): part is OpenAITextContent => part.type === 'text')
      .map(part => part.text)
      .join('\n');
  } else {
    content = '';
  }

  return {
    role: openAIRoleToA3S(msg.role),
    content,
    metadata: msg.name ? { name: msg.name } : undefined,
  };
}

/**
 * Convert array of OpenAI messages to A3S Messages
 */
export function openAIMessagesToA3S(messages: OpenAIMessage[]): Message[] {
  return messages.map(openAIMessageToA3S);
}

/**
 * Convert OpenAI tool call to A3S ToolCall
 */
export function openAIToolCallToA3S(toolCall: OpenAIToolCall): ToolCall {
  return {
    id: toolCall.id,
    name: toolCall.function.name,
    arguments: toolCall.function.arguments,
  };
}

// ============================================================================
// Conversion Functions: A3S -> OpenAI
// ============================================================================

/**
 * Convert A3S MessageRole to OpenAI role
 * Since proto now uses OpenAI-compatible string roles, this is a simple passthrough
 */
export function a3sRoleToOpenAI(role: MessageRole): OpenAIRole {
  switch (role) {
    case 'user':
      return 'user';
    case 'assistant':
      return 'assistant';
    case 'system':
      return 'system';
    case 'tool':
      return 'tool';
    default:
      return 'user';
  }
}

/**
 * Convert A3S Message to OpenAI message
 */
export function a3sMessageToOpenAI(msg: Message): OpenAIMessage {
  const openAIMsg: OpenAIMessage = {
    role: a3sRoleToOpenAI(msg.role),
    content: msg.content,
  };

  if (msg.metadata?.name) {
    openAIMsg.name = msg.metadata.name;
  }

  return openAIMsg;
}

/**
 * Convert A3S ToolCall to OpenAI tool call
 */
export function a3sToolCallToOpenAI(toolCall: ToolCall): OpenAIToolCall {
  return {
    id: toolCall.id,
    type: 'function',
    function: {
      name: toolCall.name,
      arguments: toolCall.arguments,
    },
  };
}

/**
 * Convert A3S FinishReason to OpenAI finish_reason
 * Since proto now uses OpenAI-compatible string values, this is a simple passthrough
 */
export function a3sFinishReasonToOpenAI(
  reason: FinishReason
): 'stop' | 'length' | 'tool_calls' | 'content_filter' | null {
  switch (reason) {
    case 'stop':
      return 'stop';
    case 'length':
      return 'length';
    case 'tool_calls':
      return 'tool_calls';
    case 'content_filter':
      return 'content_filter';
    default:
      return null;
  }
}

/**
 * Convert A3S Usage to OpenAI usage
 */
export function a3sUsageToOpenAI(usage?: Usage): OpenAIUsage | undefined {
  if (!usage) return undefined;

  return {
    prompt_tokens: usage.promptTokens || 0,
    completion_tokens: usage.completionTokens || 0,
    total_tokens: usage.totalTokens || 0,
  };
}

/**
 * Convert A3S GenerateResponse to OpenAI ChatCompletion
 */
export function a3sResponseToOpenAI(
  response: GenerateResponse,
  model: string = 'unknown'
): OpenAIChatCompletion {
  const message: OpenAIMessage = response.message
    ? a3sMessageToOpenAI(response.message)
    : { role: 'assistant', content: '' };

  // Add tool calls if present
  if (response.toolCalls && response.toolCalls.length > 0) {
    message.tool_calls = response.toolCalls.map(a3sToolCallToOpenAI);
  }

  return {
    id: `chatcmpl-${response.sessionId}`,
    object: 'chat.completion',
    created: Math.floor(Date.now() / 1000),
    model,
    choices: [
      {
        index: 0,
        message,
        finish_reason: a3sFinishReasonToOpenAI(response.finishReason),
      },
    ],
    usage: a3sUsageToOpenAI(response.usage),
  };
}

/**
 * Convert A3S GenerateChunk to OpenAI ChatCompletionChunk
 * Since proto now uses OpenAI-compatible string values, conversion is simplified
 */
export function a3sChunkToOpenAI(
  chunk: GenerateChunk,
  model: string = 'unknown'
): OpenAIChatCompletionChunk {
  const delta: OpenAIDelta = {};

  if (chunk.type === 'content' && chunk.content) {
    delta.content = chunk.content;
  }

  if (chunk.type === 'tool_call' && chunk.toolCall) {
    delta.tool_calls = [
      {
        id: chunk.toolCall.id,
        type: 'function',
        function: {
          name: chunk.toolCall.name,
          arguments: chunk.toolCall.arguments,
        },
      },
    ];
  }

  let finishReason: 'stop' | 'length' | 'tool_calls' | 'content_filter' | null =
    null;
  if (chunk.type === 'done' || chunk.finishReason) {
    finishReason = chunk.finishReason ? a3sFinishReasonToOpenAI(chunk.finishReason) : 'stop';
  }

  return {
    id: `chatcmpl-${chunk.sessionId}`,
    object: 'chat.completion.chunk',
    created: Math.floor(Date.now() / 1000),
    model,
    choices: [
      {
        index: 0,
        delta,
        finish_reason: finishReason,
      },
    ],
  };
}

// ============================================================================
// Helper Functions
// ============================================================================

/**
 * Check if a message uses OpenAI format (lowercase role)
 * Since proto now uses OpenAI-compatible string roles, all messages are in OpenAI format
 */
export function isOpenAIFormat(
  msg: Message | OpenAIMessage
): msg is OpenAIMessage {
  const role = msg.role as string;
  // Both A3S and OpenAI now use the same lowercase role strings
  return ['user', 'assistant', 'system', 'tool', 'function'].includes(role);
}

/**
 * Normalize message to A3S format (accepts both OpenAI and A3S formats)
 * Since proto now uses OpenAI-compatible string roles, this is mostly a passthrough
 */
export function normalizeMessage(msg: Message | OpenAIMessage): Message {
  if (isOpenAIFormat(msg)) {
    return openAIMessageToA3S(msg);
  }
  return msg as Message;
}

/**
 * Normalize array of messages to A3S format
 */
export function normalizeMessages(
  messages: (Message | OpenAIMessage)[]
): Message[] {
  return messages.map(normalizeMessage);
}

// ============================================================================
// Type Guards
// ============================================================================

/**
 * Type guard for OpenAI text content
 */
export function isTextContent(
  content: OpenAIContentPart
): content is OpenAITextContent {
  return content.type === 'text';
}

/**
 * Type guard for OpenAI image content
 */
export function isImageContent(
  content: OpenAIContentPart
): content is OpenAIImageContent {
  return content.type === 'image_url';
}
