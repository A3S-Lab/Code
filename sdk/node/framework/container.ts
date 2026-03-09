/**
 * Dependency Injection Container
 *
 * Provides a lightweight DI container inspired by NestJS.
 * Supports three scopes: Singleton, Scoped, and Transient.
 */

import 'reflect-metadata';

export enum Scope {
  SINGLETON = 'singleton',
  SCOPED = 'scoped',
  TRANSIENT = 'transient',
}

interface ProviderInfo {
  class: any;
  scope: Scope;
  args: any[];
}

interface ProviderMetadata {
  dependencies: any[];
}

export class DIContainer {
  private providers: Map<any, ProviderInfo> = new Map();
  private singletons: Map<any, any> = new Map();
  private scopedInstances: Map<string, Map<any, any>> = new Map();
  private metadata: Map<any, ProviderMetadata> = new Map();

  /**
   * Register a provider
   */
  register(providerClass: any, scope: Scope = Scope.SINGLETON, ...args: any[]): void {
    this.providers.set(providerClass, {
      class: providerClass,
      scope,
      args,
    });

    this.metadata.set(providerClass, {
      dependencies: this.getDependencies(providerClass),
    });
  }

  /**
   * Get constructor dependencies using reflect-metadata
   */
  private getDependencies(cls: any): any[] {
    const paramTypes = Reflect.getMetadata('design:paramtypes', cls) || [];
    return paramTypes.filter((type: any) => this.providers.has(type));
  }

  /**
   * Resolve a provider and its dependencies
   */
  resolve<T>(cls: new (...args: any[]) => T, scopeId?: string): T {
    if (!this.providers.has(cls)) {
      throw new Error(`Provider ${cls.name} not registered`);
    }

    const providerInfo = this.providers.get(cls)!;
    const { scope } = providerInfo;

    // Singleton: global singleton
    if (scope === Scope.SINGLETON) {
      if (!this.singletons.has(cls)) {
        this.singletons.set(cls, this.createInstance(cls, scopeId));
      }
      return this.singletons.get(cls)!;
    }

    // Scoped: per-scope singleton
    if (scope === Scope.SCOPED) {
      if (!scopeId) {
        throw new Error(`Scope ID required for scoped provider ${cls.name}`);
      }

      if (!this.scopedInstances.has(scopeId)) {
        this.scopedInstances.set(scopeId, new Map());
      }

      const scopedMap = this.scopedInstances.get(scopeId)!;
      if (!scopedMap.has(cls)) {
        scopedMap.set(cls, this.createInstance(cls, scopeId));
      }

      return scopedMap.get(cls)!;
    }

    // Transient: new instance every time
    return this.createInstance(cls, scopeId);
  }

  /**
   * Create an instance with dependency injection
   */
  private createInstance<T>(cls: new (...args: any[]) => T, scopeId?: string): T {
    const providerInfo = this.providers.get(cls)!;
    const providerMetadata = this.metadata.get(cls)!;

    // Recursively resolve dependencies
    const resolvedDeps = providerMetadata.dependencies.map((depType) =>
      this.resolve(depType, scopeId)
    );

    // Merge constructor args
    const allArgs = [...resolvedDeps, ...providerInfo.args];

    return new cls(...allArgs);
  }

  /**
   * Clear all scoped instances for a given scope ID
   */
  clearScope(scopeId: string): void {
    this.scopedInstances.delete(scopeId);
  }
}

/**
 * Injectable decorator
 *
 * Marks a class as injectable and sets its scope.
 *
 * @example
 * ```typescript
 * @Injectable({ scope: Scope.SINGLETON })
 * class LoggerService {
 *   log(msg: string) {
 *     console.log(`[LOG] ${msg}`);
 *   }
 * }
 * ```
 */
export function Injectable(options: { scope?: Scope } = {}): ClassDecorator {
  return (target: any) => {
    const scope = options.scope || Scope.SINGLETON;
    Reflect.defineMetadata('injectable:scope', scope, target);
    return target;
  };
}
