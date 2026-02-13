/**
 * UIMessage / ModelMessage Conversion Layer
 *
 * Vercel AI SDK-style message types for frontend ↔ backend conversion.
 *
 * - UIMessage: Frontend format with id, createdAt, parts (for rendering)
 * - ModelMessage: Backend format with role, content (for LLM)
 *
 * @example
 * ```typescript
 * import { convertToModelMessages, convertToUIMessages } from '@a3s-lab/code';
 *
 * // Frontend → Backend (before calling generateText/streamText)
 * const modelMessages = convertToModelMessages(uiMessages);
 * const { text } = await generateText({ model, messages: modelMessages });
 *
 * // Backend → Frontend (after receiving response)
 * const uiMessages = convertToUIMessages(modelMessages);
 * ```
 */

import type { Message, ToolCall } from './client.js';

// ============================================================================
// UIMessage Types (Frontend)
// ============================================================================

/** Text part in a UIMessage */
export interface UIMessageTextPart {
  type: 'text';
  text: string;
}

/** Tool invocation part in a UIMessage */
export interface UIMessageToolInvocationPart {
  type: 'tool-invocation';
  toolInvocation: {
    toolCallId: string;
    toolName: string;
    args: Record<string, unknown>;
    state: 'call' | 'result' | 'partial-call';
    result?: unknown;
  };
}

/** Step boundary part in a UIMessage */
export interface UIMessageStepBoundaryPart {
  type: 'step-boundary';
  stepIndex: number;
}

/** Reasoning part in a UIMessage */
export interface UIMessageReasoningPart {
  type: 'reasoning';
  reasoning: string;
}

/** Union of all UIMessage part types */
export type UIMessagePart =
  | UIMessageTextPart
  | UIMessageToolInvocationPart
  | UIMessageStepBoundaryPart
  | UIMessageReasoningPart;

/**
 * UIMessage — Frontend-friendly message format.
 *
 * Contains id, timestamps, and structured parts for rendering in chat UIs.
 * This is the format used by frontend hooks (like useChat in Vercel AI SDK).
 */
export interface UIMessage<TMetadata = Record<string, unknown>> {
  /** Unique message identifier */
  id: string;
  /** Message role */
  role: 'user' | 'assistant' | 'system';
  /** Message content (plain text, for backward compatibility) */
  content: string;
  /** Structured parts for rich rendering */
  parts: UIMessagePart[];
  /** Creation timestamp */
  createdAt?: Date;
  /** Custom metadata */
  metadata?: TMetadata;
}

// ============================================================================
// ModelMessage Types (Backend)
// ============================================================================

/** System message for LLM */
export interface SystemModelMessage {
  role: 'system';
  content: string;
}

/** User message for LLM */
export interface UserModelMessage {
  role: 'user';
  content: string;
}

/** Assistant message for LLM (may include tool calls) */
export interface AssistantModelMessage {
  role: 'assistant';
  content: string;
  toolCalls?: ToolCall[];
}

/** Tool result message for LLM */
export interface ToolModelMessage {
  role: 'tool';
  content: string;
  toolCallId?: string;
  toolName?: string;
}

/**
 * ModelMessage — Backend message format for LLM.
 *
 * This is the format passed to generateText(), streamText(), and the
 * underlying A3S client. It maps directly to the A3S proto Message type.
 */
export type ModelMessage =
  | SystemModelMessage
  | UserModelMessage
  | AssistantModelMessage
  | ToolModelMessage;

// ============================================================================
// Conversion: UIMessage → ModelMessage
// ============================================================================

/**
 * Convert a single UIMessage to one or more ModelMessages.
 *
 * A single UIMessage may produce multiple ModelMessages when it contains
 * tool invocations (assistant message + tool result messages).
 */
function uiMessageToModelMessages(uiMessage: UIMessage): ModelMessage[] {
  const messages: ModelMessage[] = [];

  if (uiMessage.role === 'system') {
    messages.push({ role: 'system', content: uiMessage.content });
    return messages;
  }

  if (uiMessage.role === 'user') {
    // Extract text from parts, fall back to content
    const text = uiMessage.parts
      .filter((p): p is UIMessageTextPart => p.type === 'text')
      .map(p => p.text)
      .join('\n') || uiMessage.content;

    messages.push({ role: 'user', content: text });
    return messages;
  }

  // Assistant message — may contain text + tool invocations
  if (uiMessage.role === 'assistant') {
    const textParts = uiMessage.parts.filter(
      (p): p is UIMessageTextPart => p.type === 'text',
    );
    const toolParts = uiMessage.parts.filter(
      (p): p is UIMessageToolInvocationPart => p.type === 'tool-invocation',
    );

    const text = textParts.map(p => p.text).join('\n') || uiMessage.content;

    // Build tool calls from tool invocation parts
    const toolCalls: ToolCall[] = toolParts
      .filter(p => p.toolInvocation.state === 'call' || p.toolInvocation.state === 'result')
      .map(p => ({
        id: p.toolInvocation.toolCallId,
        name: p.toolInvocation.toolName,
        arguments: JSON.stringify(p.toolInvocation.args),
        result: p.toolInvocation.state === 'result' && p.toolInvocation.result
          ? {
              success: true,
              output: typeof p.toolInvocation.result === 'string'
                ? p.toolInvocation.result
                : JSON.stringify(p.toolInvocation.result),
              error: '',
              metadata: {},
            }
          : undefined,
      }));

    const assistantMsg: AssistantModelMessage = {
      role: 'assistant',
      content: text,
    };
    if (toolCalls.length > 0) {
      assistantMsg.toolCalls = toolCalls;
    }
    messages.push(assistantMsg);

    // Add tool result messages for completed invocations
    for (const tp of toolParts) {
      if (tp.toolInvocation.state === 'result' && tp.toolInvocation.result !== undefined) {
        messages.push({
          role: 'tool',
          content: typeof tp.toolInvocation.result === 'string'
            ? tp.toolInvocation.result
            : JSON.stringify(tp.toolInvocation.result),
          toolCallId: tp.toolInvocation.toolCallId,
          toolName: tp.toolInvocation.toolName,
        });
      }
    }
  }

  return messages;
}

/**
 * Convert UIMessage[] to ModelMessage[] for use with generateText/streamText.
 *
 * This is the primary conversion function for frontend → backend message flow.
 *
 * @example
 * ```typescript
 * import { convertToModelMessages, generateText } from '@a3s-lab/code';
 *
 * // In your API route handler:
 * const modelMessages = convertToModelMessages(uiMessages);
 * const { text } = await generateText({
 *   model: openai('gpt-4o'),
 *   messages: modelMessages,
 * });
 * ```
 */
export function convertToModelMessages(uiMessages: UIMessage[]): ModelMessage[] {
  return uiMessages.flatMap(uiMessageToModelMessages);
}

// ============================================================================
// Conversion: ModelMessage → UIMessage
// ============================================================================

let _idCounter = 0;
function generateId(): string {
  return `msg-${Date.now()}-${++_idCounter}`;
}

/**
 * Convert ModelMessage[] to UIMessage[] for frontend rendering.
 *
 * Groups assistant messages with their tool results into single UIMessages
 * with structured parts.
 *
 * @example
 * ```typescript
 * import { convertToUIMessages } from '@a3s-lab/code';
 *
 * const uiMessages = convertToUIMessages(response.messages);
 * // Render uiMessages in your chat UI
 * ```
 */
export function convertToUIMessages(modelMessages: ModelMessage[]): UIMessage[] {
  const uiMessages: UIMessage[] = [];
  let i = 0;

  while (i < modelMessages.length) {
    const msg = modelMessages[i];

    if (msg.role === 'system') {
      uiMessages.push({
        id: generateId(),
        role: 'system',
        content: msg.content,
        parts: [{ type: 'text', text: msg.content }],
        createdAt: new Date(),
      });
      i++;
      continue;
    }

    if (msg.role === 'user') {
      uiMessages.push({
        id: generateId(),
        role: 'user',
        content: msg.content,
        parts: [{ type: 'text', text: msg.content }],
        createdAt: new Date(),
      });
      i++;
      continue;
    }

    if (msg.role === 'assistant') {
      const parts: UIMessagePart[] = [];

      // Add text part
      if (msg.content) {
        parts.push({ type: 'text', text: msg.content });
      }

      // Add tool invocation parts
      if (msg.toolCalls) {
        for (const tc of msg.toolCalls) {
          const args = tc.arguments ? JSON.parse(tc.arguments) : {};

          // Look ahead for matching tool result
          let result: unknown = undefined;
          let state: 'call' | 'result' = 'call';
          for (let j = i + 1; j < modelMessages.length; j++) {
            const next = modelMessages[j];
            if (next.role === 'tool' && next.toolCallId === tc.id) {
              try {
                result = JSON.parse(next.content);
              } catch {
                result = next.content;
              }
              state = 'result';
              break;
            }
            if (next.role !== 'tool') break;
          }

          parts.push({
            type: 'tool-invocation',
            toolInvocation: {
              toolCallId: tc.id,
              toolName: tc.name,
              args,
              state,
              result,
            },
          });
        }
      }

      uiMessages.push({
        id: generateId(),
        role: 'assistant',
        content: msg.content,
        parts,
        createdAt: new Date(),
      });
      i++;

      // Skip consumed tool result messages
      while (i < modelMessages.length && modelMessages[i].role === 'tool') {
        i++;
      }
      continue;
    }

    // Standalone tool message (shouldn't happen normally, but handle gracefully)
    if (msg.role === 'tool') {
      i++;
      continue;
    }

    i++;
  }

  return uiMessages;
}

// ============================================================================
// Conversion: A3S Message ↔ ModelMessage
// ============================================================================

/**
 * Convert A3S Message to ModelMessage.
 */
export function a3sMessageToModel(msg: Message): ModelMessage {
  switch (msg.role) {
    case 'system':
      return { role: 'system', content: msg.content };
    case 'user':
      return { role: 'user', content: msg.content };
    case 'assistant':
      return { role: 'assistant', content: msg.content };
    case 'tool':
      return { role: 'tool', content: msg.content };
    default:
      return { role: 'user', content: msg.content };
  }
}

/**
 * Convert ModelMessage to A3S Message.
 */
export function modelMessageToA3s(msg: ModelMessage): Message {
  return {
    role: msg.role,
    content: msg.content,
  };
}

/**
 * Convert A3S Message[] to ModelMessage[].
 */
export function a3sMessagesToModel(messages: Message[]): ModelMessage[] {
  return messages.map(a3sMessageToModel);
}

/**
 * Convert ModelMessage[] to A3S Message[].
 */
export function modelMessagesToA3s(messages: ModelMessage[]): Message[] {
  return messages.map(modelMessageToA3s);
}

// ============================================================================
// Conversion: A3S Message ↔ UIMessage (shorthand)
// ============================================================================

/**
 * Convert A3S Message[] to UIMessage[] (shorthand for a3sMessagesToModel + convertToUIMessages).
 */
export function a3sMessagesToUI(messages: Message[]): UIMessage[] {
  return convertToUIMessages(a3sMessagesToModel(messages));
}

/**
 * Convert UIMessage[] to A3S Message[] (shorthand for convertToModelMessages + modelMessagesToA3s).
 */
export function uiMessagesToA3s(uiMessages: UIMessage[]): Message[] {
  return modelMessagesToA3s(convertToModelMessages(uiMessages));
}
