/**
 * A3S Code Framework - NestJS-inspired framework for TypeScript
 *
 * Provides:
 * - Dependency Injection (DI Container)
 * - Decorators (@Injectable, @Middleware, @Guard)
 * - AgentFactory (NestJS-like factory pattern)
 * - High-level abstractions (Guards, Interceptors, Pipes, Filters)
 */

export { DIContainer, Injectable, Scope } from './container';
export { Middleware, Guard, Interceptor, Pipe, ExceptionFilter } from './decorators';
export { AgentFactory } from './factory';
export { AgentSession } from './session';
export {
  MiddlewareAdapter,
  GuardMiddleware,
  InterceptorMiddleware,
  PipeMiddleware,
  FilterMiddleware,
  MiddlewareResult,
} from './adapters';

export const VERSION = '0.1.0';
