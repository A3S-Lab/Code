import {
  defineConfig,
  js,
  ts,
  reactPlugin,
  reactHooksPlugin,
} from '@rslint/core';

export default defineConfig([
  js.configs.recommended,
  ts.configs.recommended,
  reactPlugin.configs.recommended,
  reactHooksPlugin.configs.recommended,
]);
