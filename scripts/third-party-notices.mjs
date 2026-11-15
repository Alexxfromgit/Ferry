// Regenerate THIRD_PARTY_NOTICES.md: `node scripts/third-party-notices.mjs` from the repo root.
// Lists every Rust crate in the lock file, every npm package bundled into the app and every
// Android library in the Android app, by license.
//
// The Android list comes from Gradle, so it needs the Android project (app/src-tauri/gen/android)
// and a JDK Gradle accepts (JAVA_HOME). Without them, the Android list already in
// THIRD_PARTY_NOTICES.md is kept.
import { execFileSync, execSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const meta = JSON.parse(
  execSync("cargo metadata --format-version 1 --locked", { maxBuffer: 1 << 28 }).toString(),
);
const ours = new Set(meta.workspace_members);
const byLicense = new Map();
const add = (license, name) => {
  const key = license || "unknown (see the package)";
  if (!byLicense.has(key)) byLicense.set(key, new Set());
  byLicense.get(key).add(name);
};
for (const p of meta.packages) {
  if (!ours.has(p.id)) add(p.license, `${p.name} ${p.version}`);
}

const app = JSON.parse(readFileSync("app/package.json", "utf8"));
for (const name of Object.keys(app.dependencies ?? {})) {
  const pkg = JSON.parse(readFileSync(`app/node_modules/${name}/package.json`, "utf8"));
  add(typeof pkg.license === "string" ? pkg.license : JSON.stringify(pkg.license), `${name} ${pkg.version} (npm)`);
}

const sectionsOf = (groups) =>
  [...groups.entries()]
    .sort((a, b) => b[1].size - a[1].size)
    .map(([license, names]) => `### ${license} (${names.size})\n\n${[...names].sort().join(", ")}\n`)
    .join("\n");
const sections = sectionsOf(byLicense);

// ------------------------------------------------------------------- Android

const ANDROID_HEADING = "## Android libraries";
const androidProject = "app/src-tauri/gen/android";
const gradleCache = join(process.env.GRADLE_USER_HOME ?? join(homedir(), ".gradle"), "caches/modules-2/files-2.1");

/** group:artifact:version of every library in the Play build (a superset of the Direct build). */
function androidLibraries() {
  const cwd = join(process.cwd(), androidProject);
  const args = ["-q", ":app:dependencies", "--configuration", "universalReleaseRuntimeClasspath"];
  // Windows runs the .bat wrapper through cmd.exe; elsewhere the shell script runs directly.
  const [command, commandArgs] =
    process.platform === "win32"
      ? ["cmd.exe", ["/d", "/c", join(cwd, "gradlew.bat"), ...args]]
      : [join(cwd, "gradlew"), args];
  const out = execFileSync(command, commandArgs, {
    cwd,
    env: { ...process.env, ORG_GRADLE_PROJECT_ferryDist: "play" },
    maxBuffer: 1 << 26,
    stdio: ["ignore", "pipe", "pipe"],
  }).toString();
  const found = new Set();
  for (const line of out.split("\n")) {
    const m = line.match(/[+\\]--- ([\w.-]+):([\w.-]+):(\S+)(?: -> (\S+))?/);
    if (!m) continue;
    const version = (m[4] ?? m[3]).replace(/[{}]/g, "").replace(/^strictly /, "");
    found.add(`${m[1]}:${m[2]}:${version}`);
  }
  if (found.size === 0) throw new Error("Gradle listed no Android libraries");
  return [...found];
}

function pomOf(group, artifact, version) {
  const dir = join(gradleCache, group, artifact, version);
  if (!existsSync(dir)) return null;
  for (const hash of readdirSync(dir)) {
    const pom = join(dir, hash, `${artifact}-${version}.pom`);
    if (existsSync(pom)) return readFileSync(pom, "utf8");
  }
  return null;
}

/** License names from a POM, following parent POMs that declare them instead. */
function pomLicenses(group, artifact, version, depth = 0) {
  const pom = group && artifact && version ? pomOf(group, artifact, version) : null;
  if (!pom) return [];
  const names = [...pom.matchAll(/<license>\s*<name>([^<]+)<\/name>/g)].map((m) => m[1].trim());
  if (names.length || depth > 4) return names;
  const parent = pom.match(/<parent>([\s\S]*?)<\/parent>/)?.[1];
  const tag = (name) => parent?.match(new RegExp(`<${name}>([^<]+)</${name}>`))?.[1].trim();
  return parent ? pomLicenses(tag("groupId"), tag("artifactId"), tag("version"), depth + 1) : [];
}

const LICENSE_NAMES = [
  [/apache.*2/i, "Apache-2.0"],
  [/^(the )?mit( license)?$/i, "MIT"],
  [/bsd.*3|new bsd/i, "BSD-3-Clause"],
];
const normalize = (name) => LICENSE_NAMES.find(([re]) => re.test(name))?.[1] ?? name;

function androidSection() {
  const groups = new Map();
  for (const coords of androidLibraries()) {
    const [group, artifact, version] = coords.split(":");
    const licenses = [...new Set(pomLicenses(group, artifact, version).map(normalize))];
    const key = licenses.length ? licenses.join(" AND ") : "unknown (see the library)";
    if (!groups.has(key)) groups.set(key, new Set());
    groups.get(key).add(`${group}:${artifact} ${version}`);
  }
  return `${ANDROID_HEADING}

The Android app also ships these Java and Kotlin libraries from Maven. The list is for the Google
Play build; the Direct build (GitHub and ferry.xenvaultinteractive.com) has the same ones except
Google Play's in-app updates. Google Play services, ML Kit (the QR code scanner, which runs on
the phone) and Google Play's in-app updates are under Google's own terms, named below. Generated
from Gradle's resolved dependencies and the license each library publishes.

${sectionsOf(groups)}`;
}

let android;
try {
  android = androidSection();
} catch (err) {
  const previous = existsSync("THIRD_PARTY_NOTICES.md") ? readFileSync("THIRD_PARTY_NOTICES.md", "utf8") : "";
  const at = previous.indexOf(ANDROID_HEADING);
  android = at >= 0 ? `${previous.slice(at).trimEnd()}\n` : "";
  console.warn(`kept the previous Android list (Gradle didn't run: ${String(err.message).split("\n")[0]})`);
}

writeFileSync(
  "THIRD_PARTY_NOTICES.md",
  `# Third-party notices

Ferry is MIT licensed. It is built from the open-source components below, each under its own
license. The full license texts are in each package's source (crates.io / npm / Maven).

## Bundled font

The app ships the **Recursive** typeface (\`@fontsource-variable/recursive\`), © The Recursive
Project Authors, licensed under the **SIL Open Font License 1.1**
(https://openfontlicense.org). The font may be bundled and redistributed with software; it may
not be sold on its own.

## Artwork

The app icon (\`app/src-tauri/icons/source.png\`), the .ferry file icon
(\`app/src-tauri/icons/ferry-file-source.png\`) and the README banner (\`branding/ferry-readme.png\`)
are supplied by the project author and are not covered by the licenses below.

## Libraries by license

Generated by \`scripts/third-party-notices.mjs\` from \`Cargo.lock\` and the app's runtime npm
dependencies (build-only tools are not shipped). An entry like "MIT OR Apache-2.0" means either
license may be chosen.

${sections}${android ? `\n${android}` : ""}`,
);
console.log(`wrote THIRD_PARTY_NOTICES.md (${byLicense.size} license groups${android ? ", Android libraries included" : ""})`);
