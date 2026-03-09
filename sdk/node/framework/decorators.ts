/**
 * Decorators for middleware, guards, interceptors, pipes, and filters
 *
 * Provides NestJS-style decorators for marking classes.
 */

/**
 * Middleware decorator
 */
export function Middleware(): ClassDecorator {
  return (target: any) => {
    Reflect.defineMetadata('is:middleware', true, target);
    return target;
  };
}

/**
 * Guard decorator
 */
export function Guard(): ClassDecorator {
  return (target: any) => {
    Reflect.defineMetadata('is:guard', true, target);
    return target;
  };
}

/**
 * Interceptor decorator
 */
export function Interceptor(): ClassDecorator {
  return (target: any) => {
    Reflect.defineMetadata('is:interceptor', true, target);
    return target;
  };
}

/**
 * Pipe decorator
 */
export function Pipe(): ClassDecorator {
  return (target: any) => {
    Reflect.defineMetadata('is:pipe', true, target);
    return target;
  };
}

/**
 * Exception Filter decorator
 */
export function ExceptionFilter(): ClassDecorator {
  return (target: any) => {
    Reflect.defineMetadata('is:exception_filter', true, target);
    return target;
  };
}
