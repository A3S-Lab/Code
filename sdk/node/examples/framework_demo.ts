/**
 * Deprecated framework demo placeholder.
 *
 * Historical versions of the repository referenced a higher-level DI/decorator
 * framework module from this file. That module is not part of the current
 * published Node SDK, so this example now fails fast with a clear message
 * instead of importing missing code.
 */

function main(): never {
  throw new Error(
    [
      'examples/framework_demo.ts is deprecated.',
      'The old framework/DI decorator API is not shipped in the current Node SDK.',
      'Use the supported SDK entrypoints from ../index.js instead.',
    ].join(' ')
  );
}

main();
