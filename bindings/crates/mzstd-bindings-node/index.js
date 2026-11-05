"use strict";

const { platform, arch } = process;
const isMusl = () => {
  // Runtime libc metadata wins over a separately installed cross-architecture musl loader.
  try {
    const report = typeof process.report?.getReport === "function" ? process.report.getReport() : null;
    if (typeof report?.header?.glibcVersionRuntime === "string" || typeof report?.header?.glibcVersion === "string") {
      return false;
    }
    if (
      Array.isArray(report?.sharedObjects) &&
      report.sharedObjects.some(
        (path) => typeof path === "string" && (path.includes("/ld-musl-") || path.includes("/libc.musl-")),
      )
    ) {
      return true;
    }
  } catch {
    // A disabled diagnostic report must not prevent filesystem-based detection.
  }
  try {
    return require("fs").readFileSync("/usr/bin/ldd", "utf8").includes("musl");
  } catch {
    // Minimal images can omit ldd while retaining the architecture's actual loader.
  }
  const muslArch = { x64: "x86_64", arm64: "aarch64" }[process.arch];
  if (!muslArch) return false;
  for (const directory of ["/lib", "/lib64"]) {
    try {
      require("fs").statSync(`${directory}/ld-musl-${muslArch}.so.1`);
      return true;
    } catch {
      // Try the other conventional loader directory before using the GNU default.
    }
  }
  return false;
};

let nativeBinding = null;
const loadErrors = [];

function requireOptionalDependency(name) {
  try {
    return require(name);
  } catch (e) {
    loadErrors.push(`Optional dependency ${name}: ${e.message}`);
    return null;
  }
}

const tryLoadBinding = () => {
  // Local `.node` files are named after `napi.binaryName` (binary file name on disk).
  // Optional-dep packages are named after `napi.packageName` (npm subpackage names),
  // which inherits any scope prefix from the parent package.
  const targets = [
    ["linux", "x64", "gnu", "./mzstd-bindings-node.linux-x64-gnu.node", "mzstd-linux-x64-gnu"],
    ["linux", "arm64", "gnu", "./mzstd-bindings-node.linux-arm64-gnu.node", "mzstd-linux-arm64-gnu"],
    ["linux", "x64", "musl", "./mzstd-bindings-node.linux-x64-musl.node", "mzstd-linux-x64-musl"],
    ["linux", "arm64", "musl", "./mzstd-bindings-node.linux-arm64-musl.node", "mzstd-linux-arm64-musl"],
    ["darwin", "x64", null, "./mzstd-bindings-node.darwin-x64.node", "mzstd-darwin-x64"],
    ["darwin", "arm64", null, "./mzstd-bindings-node.darwin-arm64.node", "mzstd-darwin-arm64"],
    ["win32", "x64", null, "./mzstd-bindings-node.win32-x64-msvc.node", "mzstd-win32-x64-msvc"],
    ["win32", "arm64", null, "./mzstd-bindings-node.win32-arm64-msvc.node", "mzstd-win32-arm64-msvc"],
  ];

  for (const [plat, a, abi, localPath, optionalDep] of targets) {
    if (platform !== plat || arch !== a) {
      continue;
    }

    if (plat === "linux" && abi) {
      const isCurMusl = isMusl();
      if ((abi === "musl") !== isCurMusl) {
        continue;
      }
    }

    try {
      nativeBinding = require(localPath);
      if (nativeBinding) {
        return;
      }
    } catch (e) {
      loadErrors.push(e.message);
    }

    try {
      const optBinding = requireOptionalDependency(optionalDep);
      if (optBinding) {
        nativeBinding = optBinding;
        return;
      }
    } catch (e) {
      loadErrors.push(e.message);
    }
  }
};

tryLoadBinding();

if (!nativeBinding) {
  throw new Error(`Failed to load native binding for ${platform}-${arch}. Errors: ${loadErrors.join(", ")}`);
}

module.exports = require("./binding.cjs")(nativeBinding);
