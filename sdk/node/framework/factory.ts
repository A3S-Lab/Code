/**
 * AgentFactory - NestJS-inspired factory for creating agents
 *
 * Provides a fluent API for configuring agents with DI and middleware.
 */

import { v4 as uuidv4 } from 'uuid';
import { DIContainer, Scope } from './container';
import {
  GuardMiddleware,
  InterceptorMiddleware,
  PipeMiddleware,
  FilterMiddleware,
} from './adapters';
import { AgentSession } from './session';

export class AgentFactory {
  private container: DIContainer;
  private middlewareClasses: any[] = [];
  private guardClasses: any[] = [];
  private interceptorClasses: any[] = [];
  private pipeClasses: any[] = [];
  private filterClasses: any[] = [];

  // Middleware instances (for later use)
  private middlewareInstances: any[] = [];
  private guardInstances: any[] = [];
  private interceptorInstances: any[] = [];
  private pipeInstances: any[] = [];
  private filterInstances: any[] = [];

  private constructor(private configPath: string) {
    this.container = new DIContainer();
  }

  /**
   * Create a new AgentFactory instance
   */
  static create(configPath: string): AgentFactory {
    return new AgentFactory(configPath);
  }

  /**
   * Register a provider (DI)
   */
  provide<T>(providerClass: new (...args: any[]) => T, ...args: any[]): this {
    const scope =
      (Reflect.getMetadata('injectable:scope', providerClass) as Scope) || Scope.SINGLETON;
    this.container.register(providerClass, scope, ...args);
    return this;
  }

  /**
   * Register middleware (auto DI)
   */
  use<T>(middlewareClass: new (...args: any[]) => T): this {
    this.middlewareClasses.push(middlewareClass);

    // Auto-register if not already registered
    if (!this.container['providers'].has(middlewareClass)) {
      const scope =
        (Reflect.getMetadata('injectable:scope', middlewareClass) as Scope) || Scope.SINGLETON;
      this.container.register(middlewareClass, scope);
    }

    // Resolve dependencies and create instance
    const middlewareInstance = this.container.resolve(middlewareClass);
    this.middlewareInstances.push(middlewareInstance);

    return this;
  }

  /**
   * Register guard (auto DI)
   */
  useGuard<T>(guardClass: new (...args: any[]) => T): this {
    this.guardClasses.push(guardClass);

    // Auto-register if not already registered
    if (!this.container['providers'].has(guardClass)) {
      const scope =
        (Reflect.getMetadata('injectable:scope', guardClass) as Scope) || Scope.SINGLETON;
      this.container.register(guardClass, scope);
    }

    // Resolve dependencies and create instance
    const guardInstance = this.container.resolve(guardClass);
    const guardMiddleware = new GuardMiddleware(guardInstance);
    this.guardInstances.push(guardMiddleware);

    return this;
  }

  /**
   * Register interceptor (auto DI)
   */
  useInterceptor<T>(interceptorClass: new (...args: any[]) => T): this {
    this.interceptorClasses.push(interceptorClass);

    // Auto-register if not already registered
    if (!this.container['providers'].has(interceptorClass)) {
      const scope =
        (Reflect.getMetadata('injectable:scope', interceptorClass) as Scope) || Scope.SINGLETON;
      this.container.register(interceptorClass, scope);
    }

    // Resolve dependencies and create instance
    const interceptorInstance = this.container.resolve(interceptorClass);
    const interceptorMiddleware = new InterceptorMiddleware(interceptorInstance);
    this.interceptorInstances.push(interceptorMiddleware);

    return this;
  }

  /**
   * Register pipe (auto DI)
   */
  usePipe<T>(pipeClass: new (...args: any[]) => T): this {
    this.pipeClasses.push(pipeClass);

    // Auto-register if not already registered
    if (!this.container['providers'].has(pipeClass)) {
      const scope =
        (Reflect.getMetadata('injectable:scope', pipeClass) as Scope) || Scope.SINGLETON;
      this.container.register(pipeClass, scope);
    }

    // Resolve dependencies and create instance
    const pipeInstance = this.container.resolve(pipeClass);
    const pipeMiddleware = new PipeMiddleware(pipeInstance);
    this.pipeInstances.push(pipeMiddleware);

    return this;
  }

  /**
   * Register exception filter (auto DI)
   */
  useFilter<T>(filterClass: new (...args: any[]) => T): this {
    this.filterClasses.push(filterClass);

    // Auto-register if not already registered
    if (!this.container['providers'].has(filterClass)) {
      const scope =
        (Reflect.getMetadata('injectable:scope', filterClass) as Scope) || Scope.SINGLETON;
      this.container.register(filterClass, scope);
    }

    // Resolve dependencies and create instance
    const filterInstance = this.container.resolve(filterClass);
    const filterMiddleware = new FilterMiddleware(filterInstance);
    this.filterInstances.push(filterMiddleware);

    return this;
  }

  /**
   * Create a session (with DI scope)
   */
  session(workspace: string): AgentSession {
    const sessionId = uuidv4();

    return new AgentSession(sessionId, workspace, this.container, sessionId);
  }

  /**
   * Build the final agent (optional, for explicit building)
   */
  build(): any {
    // TODO: Return Rust core agent when bindings are ready
    return null;
  }
}
