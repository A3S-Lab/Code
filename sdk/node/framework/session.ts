/**
 * AgentSession - Session wrapper with DI support
 *
 * Wraps the core session with DI container and scope management.
 */

import { DIContainer } from './container';

export class AgentSession {
  private guards: any[] = [];
  private interceptors: any[] = [];
  private pipes: any[] = [];
  private filters: any[] = [];

  constructor(
    private sessionId: string,
    private workspace: string,
    private container: DIContainer,
    private scopeId: string
  ) {}

  /**
   * Run the session with a prompt
   */
  async run(prompt: string): Promise<any> {
    // TODO: Execute middleware pipeline
    // TODO: Call Rust core session.run()

    // For now, return a mock result
    return {
      text: `Mock response for: ${prompt}`,
      sessionId: this.sessionId,
      workspace: this.workspace,
    };
  }

  /**
   * Add a guard to this session
   */
  useGuard<T>(guardClass: new (...args: any[]) => T): this {
    const guardInstance = this.container.resolve(guardClass, this.scopeId);
    this.guards.push(guardInstance);
    return this;
  }

  /**
   * Add an interceptor to this session
   */
  useInterceptor<T>(interceptorClass: new (...args: any[]) => T): this {
    const interceptorInstance = this.container.resolve(interceptorClass, this.scopeId);
    this.interceptors.push(interceptorInstance);
    return this;
  }

  /**
   * Add a pipe to this session
   */
  usePipe<T>(pipeClass: new (...args: any[]) => T): this {
    const pipeInstance = this.container.resolve(pipeClass, this.scopeId);
    this.pipes.push(pipeInstance);
    return this;
  }

  /**
   * Add an exception filter to this session
   */
  useFilter<T>(filterClass: new (...args: any[]) => T): this {
    const filterInstance = this.container.resolve(filterClass, this.scopeId);
    this.filters.push(filterInstance);
    return this;
  }
}
