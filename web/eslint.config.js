// Lint for both web apps. The import-x zones enforce the feature boundaries
// (spec: Dependency direction): app -> features -> shared, and no feature
// imports another feature.
import js from "@eslint/js";
import importX from "eslint-plugin-import-x";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

const features = ["auth", "accounts", "transfers", "payees", "deposits", "loans"];

const featureZones = (app) =>
  features.map((f) => ({
    target: `./apps/${app}/src/features/${f}`,
    from: `./apps/${app}/src/features`,
    except: [`./${f}`],
    message: "A feature must not import another feature; move shared code to src/shared.",
  }));

const layerZones = (app) => [
  {
    target: `./apps/${app}/src/shared`,
    from: [`./apps/${app}/src/features`, `./apps/${app}/src/app`],
    message: "src/shared must not import features or app code.",
  },
  {
    target: `./apps/${app}/src/features`,
    from: `./apps/${app}/src/app`,
    message: "Features must not import app code.",
  },
];

export default tseslint.config(
  { ignores: ["**/dist/**", "**/node_modules/**", "packages/api-client/src/schema.ts"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: { globals: globals.browser },
    plugins: { "import-x": importX, "react-hooks": reactHooks },
    // Without a resolver for .ts files, import-x cannot resolve relative paths
    // and no-restricted-paths silently checks nothing.
    settings: { "import-x/resolver": { node: { extensions: [".ts", ".tsx", ".js"] } } },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "import-x/no-restricted-paths": [
        "error",
        { zones: [...featureZones("mobile"), ...layerZones("mobile")] },
      ],
    },
  },
);
