import { access, readFile, readdir, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const desktopRoot = path.resolve(scriptDirectory, "..");
const failures = [];

function relativePath(filePath) {
  return path.relative(desktopRoot, filePath).split(path.sep).join("/");
}

function fail(message) {
  failures.push(message);
}

function assertEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(
      `${label}: expected ${JSON.stringify(expected)}, received ${JSON.stringify(actual)}`,
    );
  }
}

function assertIncludes(content, expected, label) {
  if (!content.includes(expected)) {
    fail(`${label}: missing ${JSON.stringify(expected)}`);
  }
}

function assertExcludes(content, forbidden, label) {
  if (content.toLowerCase().includes(forbidden.toLowerCase())) {
    fail(
      `${label}: contains retired product token ${JSON.stringify(forbidden)}`,
    );
  }
}

async function read(relative) {
  return readFile(path.join(desktopRoot, relative), "utf8");
}

async function exists(relative) {
  try {
    await access(path.join(desktopRoot, relative));
    return true;
  } catch {
    return false;
  }
}

async function collectSourceFiles(directory) {
  const result = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const filePath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      result.push(...(await collectSourceFiles(filePath)));
      continue;
    }
    if ([".rs", ".ts", ".tsx"].includes(path.extname(entry.name))) {
      result.push(filePath);
    }
  }
  return result;
}

function isTestOnly(relative) {
  return (
    relative.includes("/testing/") ||
    relative.includes("/tests/") ||
    relative.includes(".test.") ||
    relative.includes(".spec.") ||
    relative.endsWith("_tests.rs")
  );
}

const tauriConfig = JSON.parse(await read("src-tauri/tauri.conf.json"));
assertEqual(tauriConfig.productName, "Carryforth", "Tauri product name");
assertEqual(
  tauriConfig.identifier,
  "xyz.block.buzz.app",
  "stable Tauri bundle identifier",
);
assertEqual(
  tauriConfig.plugins?.["deep-link"]?.desktop?.schemes,
  ["carryforth"],
  "registered Desktop deep-link schemes",
);
if (Object.hasOwn(tauriConfig.plugins ?? {}, "updater")) {
  fail("Tauri config still registers the retired updater plugin");
}
assertEqual(
  tauriConfig.bundle?.externalBin,
  [
    "binaries/buzz-acp",
    "binaries/buzz-agent",
    "binaries/buzz-dev-mcp",
    "binaries/git-credential-nostr",
    "binaries/cf",
  ],
  "stable sidecar set and cf CLI bundle",
);

const packageJson = JSON.parse(await read("package.json"));
assertEqual(packageJson.name, "carryforth", "Desktop package name");
for (const dependencies of [
  packageJson.dependencies ?? {},
  packageJson.devDependencies ?? {},
]) {
  if (Object.hasOwn(dependencies, "@tauri-apps/plugin-updater")) {
    fail("Desktop package still depends on the retired updater plugin");
  }
}

const cargoToml = await read("src-tauri/Cargo.toml");
assertIncludes(cargoToml, 'name = "buzz-desktop"', "stable Rust package name");
assertIncludes(
  cargoToml,
  'description = "Carryforth desktop app"',
  "Desktop package description",
);
assertExcludes(cargoToml, "tauri-plugin-updater", "Desktop Cargo manifest");

const productConstants = await read("src/shared/constants/product.ts");
assertIncludes(
  productConstants,
  'PRODUCT_NAME = "Carryforth"',
  "frontend product name constant",
);
assertIncludes(
  productConstants,
  'MESSAGE_DEEP_LINK_SCHEME = "carryforth"',
  "frontend deep-link scheme constant",
);

const keyring = await read("src-tauri/src/app_state_keyring.rs");
for (const service of ['"buzz-desktop"', '"buzz-desktop-dev"']) {
  assertIncludes(keyring, service, "stable keyring service");
}

const communityStorage = await read(
  "src/features/communities/communityStorage.ts",
);
for (const key of [
  '"buzz-communities"',
  '"buzz-active-community-id"',
  '"buzz-workspaces"',
  '"buzz-active-workspace-id"',
]) {
  assertIncludes(communityStorage, key, "stable browser storage coordinate");
}

const requiredAssets = [
  "public/carryforth.svg",
  "public/landing/carryforth-wordmark.svg",
  "src-tauri/icons/carryforth-source.svg",
  "src-tauri/icons/icon.png",
  "src-tauri/icons/icon.icns",
  "src-tauri/icons/icon.ico",
];
for (const relative of requiredAssets) {
  if (!(await exists(relative))) {
    fail(`required Carryforth asset is missing: ${relative}`);
    continue;
  }
  const metadata = await stat(path.join(desktopRoot, relative));
  if (metadata.size === 0) {
    fail(`required Carryforth asset is empty: ${relative}`);
  }
}

const retiredProductFiles = [
  "public/buzz.svg",
  "public/landing/buzz-wordmark.png",
  "src-tauri/icons/buzz-source.png",
  "src/builderlab.rs",
  "src-tauri/src/builderlab.rs",
  "src-tauri/src/commands/updater.rs",
  "src/features/communities/hostedCommunityApi.ts",
  "src/features/communities/ui/AddCommunityDialog.tsx",
  "src/features/communities/ui/HostedCommunityCreateFlow.tsx",
  "src/features/communities/ui/HostedCommunityOnboarding.tsx",
  "src/features/settings/ui/HostedCommunitiesSettingsCard.tsx",
  "src/features/settings/ui/MobilePairingCard.tsx",
  "src/features/settings/UpdateChecker.tsx",
  "src/features/settings/UpdateIndicator.tsx",
  "src/features/settings/SidebarUpdateCard.tsx",
  "src/features/settings/hooks/UpdaterProvider.tsx",
  "src/features/profile/lib/nostrIdentityBinding.ts",
  "src/features/profile/ui/NostrBindConsentDialog.tsx",
];
for (const relative of retiredProductFiles) {
  if (await exists(relative)) {
    fail(`retired Buzz product file is present: ${relative}`);
  }
}

const forbiddenTokens = [
  "app.builderlab.xyz",
  "builderlab",
  "github.com/block/buzz",
  "buzz://",
  "buzz-wordmark",
  "/buzz.svg",
  "app-icon@",
  "tauri-plugin-updater",
  "@tauri-apps/plugin-updater",
];
const technicalAllowlist = new Map([
  // Mobile pairing is no longer reachable from Desktop product UI, but its
  // low-level historical parser remains until the Mobile product phase.
  ["communities.buzz.xyz", new Set(["src-tauri/src/commands/pairing.rs"])],
]);

const runtimeSources = [
  ...(await collectSourceFiles(path.join(desktopRoot, "src"))),
  ...(await collectSourceFiles(path.join(desktopRoot, "src-tauri", "src"))),
];
for (const filePath of runtimeSources) {
  const relative = relativePath(filePath);
  if (isTestOnly(relative)) {
    continue;
  }
  const content = await readFile(filePath, "utf8");
  for (const token of forbiddenTokens) {
    if (content.toLowerCase().includes(token.toLowerCase())) {
      fail(
        `${relative}: contains retired product token ${JSON.stringify(token)}`,
      );
    }
  }
  for (const [token, allowedFiles] of technicalAllowlist) {
    if (
      content.toLowerCase().includes(token.toLowerCase()) &&
      !allowedFiles.has(relative)
    ) {
      fail(`${relative}: contains non-allowlisted legacy coordinate ${token}`);
    }
  }
}

const nativeSource = await read("src-tauri/src/lib.rs");
assertExcludes(nativeSource, "buzz-desktop:", "native Desktop log prefixes");
assertExcludes(nativeSource, "builderlab", "native command registration");
assertExcludes(nativeSource, "plugin_updater", "native updater registration");

const e2eBridge = await read("src/testing/e2eBridge.ts");
assertExcludes(e2eBridge, "plugin:updater", "Desktop E2E command surface");
assertExcludes(
  e2eBridge,
  "is_auto_update_supported",
  "Desktop E2E native command surface",
);

const settingsPanels = await read(
  "src/features/settings/ui/SettingsPanels.tsx",
);
assertIncludes(
  settingsPanels,
  'if (name === "buzz") return "Carryforth"',
  "legacy theme display label",
);
assertIncludes(
  settingsPanels,
  'label: "Members"',
  "local member settings label",
);

if (failures.length > 0) {
  console.error("Carryforth Desktop product-surface check failed:\n");
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exitCode = 1;
} else {
  console.log("Carryforth Desktop product-surface check passed.");
}
